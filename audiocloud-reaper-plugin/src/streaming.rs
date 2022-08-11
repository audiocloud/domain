use libflac_sys::{
    FLAC__StreamEncoder, FLAC__StreamEncoderWriteStatus, FLAC__byte, FLAC__stream_encoder_delete,
    FLAC__stream_encoder_finish, FLAC__stream_encoder_init_stream, FLAC__stream_encoder_new,
    FLAC__stream_encoder_process, FLAC__stream_encoder_set_bits_per_sample, FLAC__stream_encoder_set_channels,
    FLAC__stream_encoder_set_sample_rate, FLAC__stream_encoder_set_streamable_subset,
};
use std::collections::VecDeque;
use std::ffi::c_void;
use std::ptr::slice_from_raw_parts;

use r8brain_rs::PrecisionProfile;
use tracing::*;

use audiocloud_api::change::PlayId;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct StreamingConfig {
    pub play_id:     PlayId,
    pub sample_rate: usize,
    pub channels:    usize,
    pub bit_depth:   usize,
}

struct AudioBuf {
    stream:   u64,
    timeline: f64,
    channels: Vec<Vec<f64>>,
}

struct Resampler {
    resamplers: Vec<(r8brain_rs::Resampler, Vec<f64>)>,
    timeline:   f64,
    stream:     u64,
    offset:     f64,
    from:       f64,
    to:         f64,
}

impl Resampler {
    pub fn new(num_channels: usize, from: f64, to: f64) -> Self {
        let max_src_len = 4096;
        let max_dst_len = (4096.0 * to / from * 2.0) as usize;
        let make_temp = || -> Vec<f64> {
            let mut rv = Vec::with_capacity(max_dst_len);
            rv.resize(max_dst_len, 0.0);
            rv
        };

        let resamplers =
            (0..num_channels).map(|_| {
                                 (r8brain_rs::Resampler::new(from, to, max_src_len, 2.0, PrecisionProfile::Bits32),
                                  make_temp())
                             })
                             .collect();

        let timeline = 0.0;
        let offset = 0.0;
        let stream = 0;

        Self { resamplers,
               timeline,
               stream,
               offset,
               from,
               to }
    }

    pub fn resample(&mut self, input: AudioBuf, out: &mut VecDeque<AudioBuf>) -> anyhow::Result<()> {
        let mut channels = vec![];

        self.timeline = input.timeline;
        self.offset -= input.channels[0].len() as f64 * self.to / self.from;

        for (ch, (resampler, temp)) in input.channels.into_iter().zip(self.resamplers.iter_mut()) {
            let size = resampler.process(&ch[..], &mut temp[..]);
            channels.push(Vec::from(&temp[..size]));
        }

        let len = channels.first().map(|v| v.len()).unwrap_or_default();

        out.push_back(AudioBuf { stream: self.stream,
                                 timeline: self.timeline + self.offset,
                                 channels });

        self.offset += len as f64;
        self.stream += len as u64;

        Ok(())
    }

    pub fn finish(&mut self, out: &mut VecDeque<AudioBuf>) -> anyhow::Result<()> {
        let mut channels = vec![];

        for (resampler, temp) in &mut self.resamplers {
            let size = resampler.flush(&mut temp[..]);
            channels.push(Vec::from(&temp[..size]));
        }

        let len = channels.first().map(|v| v.len()).unwrap_or_default();

        out.push_back(AudioBuf { channels,
                                 stream: self.stream,
                                 timeline: self.timeline + self.offset });

        Ok(())
    }
}

pub struct FlacEncoder {
    encoder:         *mut FLAC__StreamEncoder,
    internals:       Box<SharedInternals>,
    tmp_buffer:      Vec<Vec<i32>>,
    bits_per_sample: usize,
    stream_pos:      u64,
    queued_len:      usize,
}

impl Drop for FlacEncoder {
    fn drop(&mut self) {
        unsafe { FLAC__stream_encoder_delete(self.encoder) };
    }
}

impl FlacEncoder {
    #[instrument(skip_all)]
    pub fn new(sample_rate: usize, channels: usize, bits_per_sample: usize) -> Self {
        debug!(sample_rate, channels, bits_per_sample, "enter");

        unsafe {
            let encoder = FLAC__stream_encoder_new();
            assert_eq!(FLAC__stream_encoder_set_channels(encoder, channels as u32), 1);
            assert_eq!(FLAC__stream_encoder_set_bits_per_sample(encoder, bits_per_sample as u32),
                       1);
            assert_eq!(FLAC__stream_encoder_set_streamable_subset(encoder, 1), 1);
            assert_eq!(FLAC__stream_encoder_set_sample_rate(encoder, sample_rate as u32), 1);

            let mut internals = Box::new(SharedInternals { buffer: vec![] });

            assert_eq!(FLAC__stream_encoder_init_stream(encoder,
                                                        Some(write),
                                                        None,
                                                        None,
                                                        None,
                                                        internals.as_mut() as *mut SharedInternals as *mut c_void),
                       0);

            let tmp_buffer = (0..channels).map(|_| Vec::new()).collect::<Vec<_>>();

            Self { encoder,
                   internals,
                   tmp_buffer,
                   bits_per_sample,
                   queued_len: 0,
                   stream_pos: 0 }
        }
    }

    pub fn process(&mut self, data: AudioBuf, output: &mut VecDeque<EncodedBuf>) {
        let converter = match self.bits_per_sample {
            16 => |s: f64| dasp_sample::conv::f64::to_i16(s) as i32,
            24 => |s: f64| dasp_sample::conv::f64::to_i24(s).inner(),
            32 => |s: f64| dasp_sample::conv::f64::to_i32(s),
            i => panic!("Only 16, 24 and 32 bits_per_sample supported, not {i}"),
        };

        let mut pointers = vec![];
        for (input, output) in data.channels.iter().zip(self.tmp_buffer.iter_mut()) {
            output.clear();
            output.extend(input.iter().copied().map(converter));
            pointers.push(output.as_ptr());
        }

        let len = self.tmp_buffer.iter().map(Vec::len).min().unwrap_or_default();

        if len > 0 {
            unsafe {
                assert_eq!(FLAC__stream_encoder_process(self.encoder, pointers.as_ptr(), len as u32),
                           1);
            }

            self.queued_len += len;

            if !self.internals.buffer.is_empty() {
                output.push_back(EncodedBuf { data:        self.internals.buffer.clone(),
                                              stream_time: self.stream_pos,
                                              is_last:     false, });
                self.stream_pos += self.queued_len as u64;
                self.queued_len = 0;
                self.internals.buffer.clear();
            }
        }
    }

    pub fn finish(&mut self, output: &mut VecDeque<EncodedBuf>) {
        unsafe {
            assert_eq!(FLAC__stream_encoder_finish(self.encoder), 1);
        }

        output.push_back(EncodedBuf { data:        self.internals.buffer.clone(),
                                      stream_time: self.stream_pos,
                                      is_last:     true, });

        self.internals.buffer.clear();
    }
}

unsafe extern "C" fn write(_encoder: *const FLAC__StreamEncoder,
                           buffer: *const FLAC__byte,
                           bytes: usize,
                           _samples: u32,
                           _current_frame: u32,
                           client_data: *mut c_void)
                           -> FLAC__StreamEncoderWriteStatus {
    let internals = &mut *(client_data as *mut SharedInternals);
    internals.buffer
             .extend_from_slice(&*slice_from_raw_parts(buffer as *const u8, bytes));

    0
}

struct SharedInternals {
    buffer: Vec<u8>,
}

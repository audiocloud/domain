use std::collections::HashMap;
use std::mem;

use audiocloud_api::audio_engine::CompressedAudio;
use audiocloud_api::domain::streaming::DiffStamped;
use audiocloud_api::{now, DestinationPadId, PadMetering, SourcePadId};

use crate::sockets::{get_sockets_supervisor, PublishStreamingPacket};
use crate::tasks::task::TaskActor;

impl TaskActor {
    pub(crate) fn merge_input_peak_meters(&mut self, input_peak_meters: HashMap<DestinationPadId, PadMetering>) {
        for (pad_id, metering) in input_peak_meters {
            self.packet
                .node_inputs
                .entry(pad_id)
                .or_default()
                .push(DiffStamped::new(self.packet.created_at, metering));
        }
    }

    pub(crate) fn merge_output_peak_meters(&mut self, output_peak_meters: HashMap<SourcePadId, PadMetering>) {
        for (pad_id, metering) in output_peak_meters {
            self.packet
                .node_outputs
                .entry(pad_id)
                .or_default()
                .push(DiffStamped::new(self.packet.created_at, metering));
        }
    }

    pub(crate) fn push_compressed_audio(&mut self, audio: CompressedAudio) {
        if self.engine.should_be_playing(&audio.play_id) {
            self.packet.audio.push(DiffStamped::new(self.packet.created_at, audio));
        }
    }

    pub(crate) fn maybe_send_packet(&mut self) {
        let packet_age = now() - self.packet.created_at;
        let packet_num_audio_frames = self.packet.audio.len();
        let max_packet_age = chrono::Duration::milliseconds(self.opts.max_packet_age_ms as i64);

        if packet_age >= max_packet_age || packet_num_audio_frames >= self.opts.max_packet_audio_frames {
            let packet = mem::take(&mut self.packet);
            get_sockets_supervisor().do_send(PublishStreamingPacket { task_id: { self.id.clone() },
                                                                      packet:  { packet }, });
        }
    }
}

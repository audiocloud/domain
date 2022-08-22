use std::fs::File;
use std::io;
use std::os::unix::prelude::*;
use std::time::Duration;
use tracing::*;

use actix::{Actor, Context, Handler, Recipient};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use audiocloud_api::driver::{InstanceDriverCommand, InstanceDriverError};
use audiocloud_api::model::{Model, ModelParameter, ModelParameters};
use audiocloud_api::newtypes::FixedInstanceId;
use audiocloud_api::time::Timestamp;
use audiocloud_models::distopik::dual_1084;

use crate::{Command, InstanceConfig};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Config {
    #[serde(default = "Config::default_device")]
    device: String,
}

impl Config {
    fn default_device() -> String {
        "/dev/PIVO".to_string()
    }
}

const RECV_TIMEOUT: Duration = Duration::from_millis(10);

impl InstanceConfig for Config {
    fn create(self, id: FixedInstanceId) -> anyhow::Result<Recipient<Command>> {
        Ok(Dual1084::new(id, self)?.start().recipient())
    }
}

struct Dual1084 {
    id:             FixedInstanceId,
    config:         Config,
    last_update:    Timestamp,
    raw_fd:         RawFd,
    io_exp_data:    [[u16; 6]; 8],
    low_gain:       [UnipotRegion; 2],
    low_gain_param: ModelParameter,
}

struct Unipot {
    pub memory: [u16; 6],
}

// TODO: move to separate utils file, as we think there will be many more "unipot" etc drivers
struct UnipotRegion {
    bits: Vec<usize>,
}

impl UnipotRegion {
    pub fn new(bits: impl Iterator<Item = usize>) -> Self {
        Self { bits: bits.collect() }
    }

    pub fn write(&self, memory: &mut [u16], value: u16) {
        // for every i-th bit set in the value, we set self.bits[i]-th bit in `memory`
        // bits can be arbitrarily far from the beginning of the buffer, as long as there
        // is space
        for (i, bit) in self.bits.iter().copied().enumerate() {
            let bit_value = value & (1 << i);
            // TODO: implement
            // write_bit_16(memory, bit, bit_value != 0);
        }
    }
}

impl Dual1084 {
    pub fn new(id: FixedInstanceId, config: Config) -> anyhow::Result<Self> {
        let raw_fd = File::options().read(true).write(true).open("/dev/PIVO")?.into_raw_fd();
        let model = dual_1084::distopik_dual_1084_model();

        Ok(Dual1084 { id,
                      config,
                      raw_fd,
                      last_update: Utc::now(),
                      io_exp_data: [[0; 6]; 8],
                      low_gain: [UnipotRegion::new(5, 57..=63), UnipotRegion::new(6, (21..=28).rev())],
                      low_gain_param: dual_1084::low_gain() })
    }
}

impl Actor for Dual1084 {
    type Context = Context<Self>;
}

impl Handler<Command> for Dual1084 {
    type Result = Result<(), InstanceDriverError>;

    fn handle(&mut self, msg: Command, ctx: &mut Self::Context) -> Self::Result {
        match msg.command {
            InstanceDriverCommand::CheckConnection => Ok(()),
            InstanceDriverCommand::Stop
            | InstanceDriverCommand::Play { .. }
            | InstanceDriverCommand::Render { .. }
            | InstanceDriverCommand::Rewind { .. } => Err(InstanceDriverError::MediaNotPresent),
            InstanceDriverCommand::SetParameters(mut params) => {
                if let Some(low_gain) = params.remove(&dual_1084::LOW_GAIN) {
                    for (ch, value) in low_gain.into_iter().enumerate() {
                        // TODO: implement
                        // let value = restrech(value, &self.low_gain_param, 3, 128);

                        // TODO: is '3' the correct number here?
                        self.low_gain[ch].write(&mut self.io_exp_data[3], value);
                    }
                }

                // TODO: implement
                // self.write_io_expanders();

                Ok(())
            }
        }
    }
}

impl Dual1084 {
    fn set_io_expanders(&self) {
        let mut spi_data: [u32; 9] = [0; 9];
        const io_boards: [u16; 3] = [3, 1, 5];
        const io_output_address: [u16; 5] = [0x4000, 0x4200, 0x4400, 0x4600, 0x4800];
    }
}

mod ioctl {
    use super::SpiTransfer;
    use nix::{ioctl_none, ioctl_write_ptr};

    const PIVO_SPI_MAGIC: u8 = b'q';
    const PIVO_SPI_WRITE: u8 = 2;
    const PIVO_SET_DATA: u8 = 3;

    ioctl_write_ptr!(set_data_32, PIVO_SPI_MAGIC, PIVO_SET_DATA, &SpiTransfer);

    ioctl_none!(write_data_32, PIVO_SPI_MAGIC, PIVO_SPI_WRITE);
}

pub fn write_data(fd: RawFd, transfers: &SpiTransfer) -> io::Result<()> {
    unsafe { ioctl::set_data_32(fd, &transfers) }?;
    Ok(())
}

pub fn transfer_data(fd: RawFd) -> io::Result<()> {
    unsafe { ioctl::write_data_32(fd) }?;
    Ok(())
}

#[allow(non_camel_case_types)]
#[derive(Debug, Default)]
#[repr(C)]
struct SpiTransfer {
    data: [u32; 9],
}

impl SpiTransfer {
    pub fn write(buff: &[u32]) -> Self {
        SpiTransfer { data: buff.try_into().expect("slice with incorrect length"), }
    }
}

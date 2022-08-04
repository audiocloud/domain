use audiocloud_api::change::PlayId;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct StreamingConfig {
    pub play_id:     PlayId,
    pub sample_rate: usize,
    pub channels:    usize,
    pub bit_depth:   usize,
}

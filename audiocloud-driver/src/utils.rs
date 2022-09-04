use std::ops::Range;
use audiocloud_api::model::{ModelParameter, ModelValueOption, ModelValue};

pub fn db_to_gain_factor(x: f64) -> f64{
    10_f64.powf(x / 20_f64)
}

pub fn rescale(value: f64, from: Range<f64>, to: Range<f64>) -> f64 {
  let value_from = value.max(from.start) - from.start;
  let from_len = from.end - from.start;
  let to_len = to.end - to.start;
  (value_from / from_len) * to_len + to.start
}

pub fn rescale_param(value: f64, range: &ModelParameter, ch: usize, to: f64) -> f64 {
  if let ModelValueOption::Range(ModelValue::Number(from_start), ModelValue::Number(from_end)) = range.values[0] {
    let value_from = value.max(from_start) - from_start;
    let from_len = from_end - from_start;
    let to_len = to;
    return (value_from / from_len) * to_len;
  }
  else{return 0.0}
}

pub fn repoint_param(value: f64, ladder: &ModelParameter, ch: usize) -> f64 {
  ladder.values.iter().position(|x| *x == ModelValueOption::Single(ModelValue::Number(value))).unwrap() as f64
}

pub fn clamp(value: f64, to: Range<f64>) -> f64 {
  value.min(to.end).max(to.start)
}

pub fn write_bit_16(dest: &mut u16, position: u16, val: u16) {
  //  let val = val.round() as u16;
  if val != 0 {
    *dest |= 1 << position;
  } else {
    *dest &= !(1 << position);
  }
}

pub fn swap_u16(val: u16) -> u16 {
  (val << 8) | (val >> 8)
}
pub mod audiocloud_plugin;
pub mod control_surface;
pub mod events;

reaper_low::reaper_vst_plugin!();
vst::plugin_main!(audiocloud_plugin::AudiocloudPlugin);

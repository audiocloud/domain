use actix::Message;

use audiocloud_api::common::instance::{DesiredInstancePlayState, ReportInstancePlayState, ReportInstancePowerState};
use audiocloud_api::common::newtypes::FixedInstanceId;
use audiocloud_api::common::task::{InstanceParameters, InstanceReports};
use audiocloud_api::common::time::Timestamped;
use audiocloud_api::instance_driver::InstanceDriverEvent;

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct SetInstanceParameters {
    pub instance_id: FixedInstanceId,
    pub parameters:  InstanceParameters,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct SetInstanceDesiredPlayState {
    pub instance_id: FixedInstanceId,
    pub desired:     DesiredInstancePlayState,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifyInstanceReports {
    pub instance_id: FixedInstanceId,
    pub reports:     InstanceReports,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifyInstancePowerChannelsChanged {
    pub instance_id: FixedInstanceId,
    pub power:       Vec<bool>,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifyInstanceState {
    pub instance_id: FixedInstanceId,
    pub power:       Option<ReportInstancePowerState>,
    pub play:        Option<ReportInstancePlayState>,
    pub connected:   Timestamped<bool>,
}

#[derive(Message, Clone, Debug)]
#[rtype(result = "()")]
pub struct NotifyInstanceError {
    pub instance_id: FixedInstanceId,
    pub error:       String,
}

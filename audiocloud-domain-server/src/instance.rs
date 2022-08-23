use crate::service::nats::get_nats_client;
use audiocloud_api::driver::InstanceDriverCommand;
use audiocloud_api::newtypes::FixedInstanceId;

pub async fn request(fixed_instance_id: FixedInstanceId, request: InstanceDriverCommand) -> anyhow::Result<()> {
    get_nats_client().request_instance_driver(&fixed_instance_id, request)
                     .await
}

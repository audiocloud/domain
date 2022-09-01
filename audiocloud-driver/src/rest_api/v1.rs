use std::collections::HashMap;

use actix_web::error::ErrorInternalServerError;
use actix_web::{post, web, Error, Responder};
use maplit::hashmap;

use audiocloud_api::change::{PlayId, RenderId};
use audiocloud_api::driver::InstanceDriverCommand;
use audiocloud_api::model::MultiChannelValue;
use audiocloud_api::newtypes::{FixedInstanceId, ParameterId};

use crate::supervisor::get_driver_supervisor;
use crate::Command;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(set_parameters)
       .service(set_parameter)
       .service(stop)
       .service(play)
       .service(render)
       .service(rewind);
}

#[post("/{manufacturer}/{name}/{instance}/parameters")]
async fn set_parameters(path: web::Path<(String, String, String)>,
                        params: web::Json<HashMap<ParameterId, MultiChannelValue>>)
                        -> impl Responder {
    let instance_id = get_instance_id(path.into_inner());

    let command = InstanceDriverCommand::SetParameters(params.into_inner());
    let command = Command { instance_id, command };

    let rv = get_driver_supervisor().send(command)
                                    .await
                                    .map_err(ErrorInternalServerError)?;

    Ok::<_, Error>(web::Json(rv))
}

#[post("/{manufacturer}/{name}/{instance}/parameters/{parameter_id}")]
async fn set_parameter(path: web::Path<(String, String, String, ParameterId)>,
                       value: web::Json<MultiChannelValue>)
                       -> impl Responder {
    let (manufacturer, name, instance, parameter_id) = path.into_inner();
    let instance_id = get_instance_id((manufacturer, name, instance));

    let command = InstanceDriverCommand::SetParameters(hashmap! { parameter_id => value.into_inner() });
    let command = Command { instance_id, command };

    let rv = get_driver_supervisor().send(command)
                                    .await
                                    .map_err(ErrorInternalServerError)?;

    Ok::<_, Error>(web::Json(rv))
}

#[post("/{manufacturer}/{name}/{instance}/stop")]
async fn stop(path: web::Path<(String, String, String)>) -> impl Responder {
    let instance_id = get_instance_id(path.into_inner());

    let command = InstanceDriverCommand::Stop;
    let command = Command { instance_id, command };

    let rv = get_driver_supervisor().send(command)
                                    .await
                                    .map_err(ErrorInternalServerError)?;

    Ok::<_, Error>(web::Json(rv))
}

#[post("/{manufacturer}/{name}/{instance}/play/{play_id}")]
async fn play(path: web::Path<(String, String, String, PlayId)>) -> impl Responder {
    let (manufacturer, name, instance, play_id) = path.into_inner();
    let instance_id = get_instance_id((manufacturer, name, instance));

    let command = InstanceDriverCommand::Play { play_id };
    let command = Command { instance_id, command };

    let rv = get_driver_supervisor().send(command)
                                    .await
                                    .map_err(ErrorInternalServerError)?;

    Ok::<_, Error>(web::Json(rv))
}

#[post("/{manufacturer}/{name}/{instance}/render/{render_id}")]
async fn render(path: web::Path<(String, String, String, RenderId)>, length: web::Json<f64>) -> impl Responder {
    let (manufacturer, name, instance, render_id) = path.into_inner();
    let instance_id = get_instance_id((manufacturer, name, instance));
    let length = length.into_inner();

    let command = InstanceDriverCommand::Render { render_id, length };
    let command = Command { instance_id, command };

    let rv = get_driver_supervisor().send(command)
                                    .await
                                    .map_err(ErrorInternalServerError)?;

    Ok::<_, Error>(web::Json(rv))
}

#[post("/{manufacturer}/{name}/{instance}/rewind")]
async fn rewind(path: web::Path<(String, String, String)>, to: web::Json<f64>) -> impl Responder {
    let instance_id = get_instance_id(path.into_inner());

    let command = InstanceDriverCommand::Rewind { to: to.into_inner() };
    let command = Command { instance_id, command };

    let rv = get_driver_supervisor().send(command)
                                    .await
                                    .map_err(ErrorInternalServerError)?;

    Ok::<_, Error>(web::Json(rv))
}

fn get_instance_id((manufacturer, name, instance): (String, String, String)) -> FixedInstanceId {
    FixedInstanceId::new(manufacturer, name, instance)
}

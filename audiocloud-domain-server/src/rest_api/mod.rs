use actix::fut;
use std::convert::Infallible;
use std::future::Future;

use actix::fut::Ready;
use actix_web::body::{BoxBody, EitherBody};
use actix_web::dev::Payload;
use actix_web::{get, web, FromRequest, HttpRequest, HttpResponse, HttpResponseBuilder, Responder};
use reqwest::StatusCode;
use serde::Serialize;
use serde_json::json;

use crate::ResponseMedia;
use audiocloud_api::domain::DomainError;
use audiocloud_api::{Codec, Json, MsgPack};

mod v1;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(healthz);
    cfg.service(web::scope("/v1").configure(v1::configure));
}

#[get("/healthz")]
async fn healthz() -> impl Responder {
    let res = json!({
      "healthy": true
    });

    web::Json(res)
}

pub struct ApiResponder(ResponseMedia);

impl ApiResponder {
    pub async fn respond<T, F>(self, fut: F) -> ApiResponse<T>
        where T: Serialize,
              F: Future<Output = Result<T, DomainError>>
    {
        let rv = fut.await;
        ApiResponse(self.0, rv)
    }
}

impl FromRequest for ApiResponder {
    type Error = Infallible;
    type Future = Ready<Result<Self, Infallible>>;

    fn from_request(req: &HttpRequest, _payload: &mut Payload) -> Self::Future {
        let rv = Self(req.headers()
                         .get("Accept")
                         .and_then(|v| {
                             if let Ok(v) = v.to_str() {
                                 if v == mime::APPLICATION_MSGPACK.essence_str() {
                                     Some(ResponseMedia::MsgPack)
                                 } else if v == mime::APPLICATION_JSON.essence_str() {
                                     Some(ResponseMedia::Json)
                                 } else {
                                     None
                                 }
                             } else {
                                 None
                             }
                         })
                         .unwrap_or(ResponseMedia::Json));

        fut::ready(Ok(rv))
    }
}

pub struct ApiResponse<T>(ResponseMedia, Result<T, DomainError>);

impl<T> Responder for ApiResponse<T> where T: Serialize
{
    type Body = EitherBody<BoxBody>;

    fn respond_to(self, _req: &HttpRequest) -> HttpResponse<Self::Body> {
        let err_resp = |err: DomainError| {
            let (content, content_type) = match self.0 {
                ResponseMedia::Json => (Json.serialize(&err).unwrap(), mime::APPLICATION_JSON.as_ref()),
                ResponseMedia::MsgPack => (MsgPack.serialize(&err).unwrap(), mime::APPLICATION_MSGPACK.as_ref()),
            };

            let status = StatusCode::from_u16(err.get_status()).unwrap();
            HttpResponseBuilder::new(status).content_type(content_type)
                                            .body(content)
                                            .map_into_right_body()
        };

        match self.1 {
            Ok(ok) => {
                let (content, content_type) = match self.0 {
                    ResponseMedia::Json => (Json.serialize(&ok)
                                                .map_err(|e| DomainError::Serialization { error: e.to_string() }),
                                            mime::APPLICATION_JSON.as_ref()),
                    ResponseMedia::MsgPack => {
                        (MsgPack.serialize(&ok)
                                .map_err(|e| DomainError::Serialization { error: e.to_string() }),
                         mime::APPLICATION_MSGPACK.as_ref())
                    }
                };

                let content = match content {
                    Err(err) => return err_resp(err),
                    Ok(content) => content,
                };

                HttpResponseBuilder::new(StatusCode::OK).content_type(content_type)
                                                        .body(content)
                                                        .map_into_left_body()
            }
            Err(err) => err_resp(err),
        }
    }
}

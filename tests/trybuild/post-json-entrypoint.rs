use serde::Serialize;
use shared_restapi::{Client, RestResponse};

#[derive(Serialize)]
struct Payload {
    ok: bool,
}

fn main() {
    let client = Client::new();
    let payload = Payload { ok: true };
    let _response: Result<RestResponse, shared_restapi::RestError> =
        client.post_json("https://api.example.com/v1/data", &payload);
}

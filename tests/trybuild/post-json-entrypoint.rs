use shared_restapi::{Client, RestResponse};

fn main() {
    let client = Client::new();
    let payload = sonic_rs::json!({"ok": true});
    let _response: Result<RestResponse, shared_restapi::RestError> =
        client.post_json("https://api.example.com/v1/data", &payload);
}

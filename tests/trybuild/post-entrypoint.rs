use shared_restapi::{Client, RestResponse};

fn main() {
    let client = Client::new();
    let _response: Result<RestResponse, shared_restapi::RestError> = client.post(
        "https://api.example.com/v1/data",
        b"{\"ok\":true}".as_slice(),
    );
}

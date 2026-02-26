use shared_restapi::{Client, RestResponse};

fn main() {
    let client = Client::new();
    let _response: Result<RestResponse, shared_restapi::RestError> = client.get_url(
        "https://api.example.com/v1/data",
    );
}

use shared_restapi::{Client, RestRequest, RestResponse};
use reqwest::Method;

fn main() {
    let client = Client::new();
    let _response: Result<RestResponse, shared_restapi::RestError> = client.execute(
        RestRequest::new(Method::GET, "https://api.example.com/v1/data"),
    );
}

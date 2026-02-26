use shared_restapi::RestRequest;

fn main() {
    let _request = RestRequest::post("https://api.example.com/v1/data")
        .with_body("{\"ok\":true}");
}

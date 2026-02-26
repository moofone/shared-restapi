use shared_restapi::Client;

fn main() {
    let client = Client::new();
    let payload = sonic_rs::json!({"ok": true});
    let _fut = client.post_json_response(
        "https://api.example.com/v1/data",
        &payload,
    );
}

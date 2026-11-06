//! Prints the protocol's JSON Schema; `schema/protocol.json` is its output.

fn main() {
    let document = rho_protocol::schema::document();
    let text = serde_json::to_string_pretty(&document).expect("a JSON value always serializes");
    println!("{text}");
}

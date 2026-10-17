fn main() {
    println!("{}", serde_json::to_string(&slicc_browsers::catalog().protected_origins).unwrap());
}

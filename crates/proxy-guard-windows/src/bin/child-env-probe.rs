use std::collections::BTreeMap;

fn main() {
    let names = [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "http_proxy",
        "https_proxy",
        "NO_PROXY",
        "no_proxy",
        "ALL_PROXY",
        "all_proxy",
        "PATH",
    ];
    let values = names
        .into_iter()
        .map(|name| (name, std::env::var(name).ok()))
        .collect::<BTreeMap<_, _>>();
    println!("{}", serde_json::to_string(&values).expect("serialize env"));
}

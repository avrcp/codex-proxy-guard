//! CodexProxyGuard entry point: Unicode `wmain` equivalent over
//! `std::env::args_os`, one strict parse, one dispatch.

mod build_info;
mod cli;
mod licenses;
mod worker;

fn main() {
    let arguments: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    let code = cli::run(&arguments);
    std::process::exit(code);
}

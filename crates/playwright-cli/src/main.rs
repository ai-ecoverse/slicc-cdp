fn main() {
    #[cfg(windows)]
    slicc_cdp::remove_parked_binary();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cdp_env = std::env::var("SLICC_CDP_URL").ok();
    let session_env = std::env::var("PLAYWRIGHT_CLI_SESSION").ok();
    let output = slicc_cdp::execute_with_session(
        &args,
        cdp_env.as_deref(),
        session_env.as_deref(),
    );
    print!("{}", output.stdout);
    eprint!("{}", output.stderr);
    std::process::exit(output.code);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cdp_env = std::env::var("SLICC_CDP_URL").ok();
    let output = curlwright::execute(&args, cdp_env.as_deref());
    print!("{}", output.stdout);
    eprint!("{}", output.stderr);
    std::process::exit(output.code);
}

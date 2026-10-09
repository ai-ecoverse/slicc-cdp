fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cdp_env = std::env::var("SLICC_CDP_URL").ok();
    let output = curlwright::execute(&args, cdp_env.as_deref());
    let mut stdout = std::io::stdout().lock();
    let _ = std::io::Write::write_all(&mut stdout, &output.stdout);
    let _ = std::io::Write::flush(&mut stdout);
    eprint!("{}", output.stderr);
    std::process::exit(output.code);
}

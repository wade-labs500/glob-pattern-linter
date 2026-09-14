use std::env;
use std::process::ExitCode;

use globlint::pretty_print;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: globlint <pattern> [pattern...]");
        return ExitCode::FAILURE;
    }

    let mut all_ok = true;
    for pattern in &args {
        match pretty_print(pattern) {
            Ok(canonical) if &canonical == pattern => println!("{}", pattern),
            Ok(canonical) => println!("{} => {}", pattern, canonical),
            Err(e) => {
                eprintln!("{}: invalid: {}", pattern, e);
                all_ok = false;
            }
        }
    }

    if all_ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

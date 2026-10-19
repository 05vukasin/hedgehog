use std::process::ExitCode;

const USAGE: &str = "usage: hedgehog describe <file>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["describe", path] => match hh_core::open(path) {
            Ok(wb) => {
                print!("{}", hh_core::describe(&wb));
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{path}: {e}");
                ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

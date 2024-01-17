use pixi::cli;

pub fn main() {
    if let Err(err) = cli::execute() {
        eprintln!("{err:?}");
        std::process::exit(1);
    }
}

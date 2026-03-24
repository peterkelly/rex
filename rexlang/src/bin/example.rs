#[tokio::main]
async fn main() {
    match rexlang::eval("1 + 2").await {
        Ok(res) => {
            println!("{}", res);
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}

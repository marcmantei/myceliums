macro_rules! create_handler {
    ($name:ident, $body:expr) => {
        fn $name() -> String {
            $body
        }
    };
}

macro_rules! log_message {
    ($level:expr, $msg:expr) => {
        println!("[{}] {}", $level, $msg);
    };
}

fn main() {
    println!("Starting application");
    let items = vec![1, 2, 3];
    let formatted = format!("items: {:?}", items);
    eprintln!("debug: {}", formatted);
}

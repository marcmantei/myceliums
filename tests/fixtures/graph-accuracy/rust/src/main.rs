mod auth;
mod config;
mod model;
mod report;
mod service;
mod store;
mod utils;
mod validation;

use config::Config;
use model::{format_name, Item, Order, User};
use report::{publish, ConsoleSink, OrderReport};
use service::{OrderService, UserService};
use store::Repo;

fn handler(event: &[String]) -> String {
    format!("handled {}", event.len())
}

fn dispatch(event: &[String]) -> String {
    let handler = |queued: &[String]| format!("queued {}", queued.len());
    handler(event)
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    println!("{}", run(&argv));
}

fn run(argv: &[String]) -> String {
    let mut users = UserService::new(Repo::<User>::new());
    let mut orders = OrderService::new(Repo::<Order>::new());
    let user = match users.register(&argv[0], &argv[1]) {
        Ok(user) => user,
        Err(error) => return error,
    };
    orders.place(user, vec![Item { price: 3 }]);
    println!("{}", format_name(&argv[0]));
    dispatch(argv);
    handler(argv);
    let report = OrderReport::new(Vec::new());
    publish(&report.report, &mut ConsoleSink);
    let _theme = Config::load("theme=dark").theme();
    Config::new(std::collections::HashMap::new()).get("argv", "")
}

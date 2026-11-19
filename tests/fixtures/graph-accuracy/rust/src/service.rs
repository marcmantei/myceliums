use crate::auth::parse as parse_token;
use crate::auth::{refresh, Token};
use crate::config::{self, Config};
use crate::model::{Item, Order, User};
use crate::store::Repo;
use crate::utils::map;
use crate::validation::{is_email, Validator};

pub struct UserService {
    repo: Repo<User>,
    validator: Validator,
}

impl UserService {
    pub fn new(repo: Repo<User>) -> Self {
        UserService {
            repo,
            validator: Validator::new(vec![is_email]),
        }
    }

    pub fn register(&mut self, name: &str, email: &str) -> Result<User, String> {
        if !self.validator.validate(email) {
            return Err("invalid email".to_string());
        }
        let user = User::new(name, email);
        Ok(user.save(&mut self.repo))
    }

    pub fn login(&self, raw: &str) -> Option<Token> {
        let token = parse_token(raw).ok()?;
        refresh(token, 0)
    }

    pub fn settings(&self, text: &str) -> String {
        let cfg = Config::load(text);
        cfg.get("theme", "light")
    }

    pub fn get(&self, index: usize) -> Option<User> {
        self.repo.get(index)
    }

    pub fn names(&self, users: &[User]) -> Vec<String> {
        map(users, |user| user.display_name())
    }
}

pub struct OrderService {
    repo: Repo<Order>,
}

impl OrderService {
    pub fn new(repo: Repo<Order>) -> Self {
        OrderService { repo }
    }

    pub fn place(&mut self, user: User, items: Vec<Item>) -> u32 {
        let order = Order::new(user, items);
        order.save(&mut self.repo);
        order.total()
    }

    pub fn parse(&self, text: &str) -> String {
        let values = config::parse(text);
        values
            .get("currency")
            .cloned()
            .unwrap_or_else(|| "EUR".to_string())
    }
}

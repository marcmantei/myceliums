use crate::store::Repo;

#[derive(Clone, Debug)]
pub struct Item {
    pub price: u32,
}

#[derive(Clone, Debug)]
pub struct User {
    pub name: String,
    pub email: String,
}

impl User {
    pub fn new(name: &str, email: &str) -> Self {
        User {
            name: name.to_string(),
            email: email.to_string(),
        }
    }

    pub fn save(self, repo: &mut Repo<User>) -> User {
        repo.add(self.clone());
        self
    }

    pub fn display_name(&self) -> String {
        format_name(&self.name)
    }
}

#[derive(Clone, Debug)]
pub struct Order {
    pub user: User,
    pub items: Vec<Item>,
}

impl Order {
    pub fn new(user: User, items: Vec<Item>) -> Self {
        Order { user, items }
    }

    pub fn save(&self, repo: &mut Repo<Order>) {
        repo.add(self.clone());
    }

    pub fn total(&self) -> u32 {
        self.items.iter().map(|item| item.price).sum()
    }

    pub fn add(&mut self, item: Item) {
        self.items.push(item);
    }
}

pub fn format_name(name: &str) -> String {
    name.trim().to_uppercase()
}

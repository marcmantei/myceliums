pub struct Repo<T> {
    rows: Vec<T>,
}

impl<T: Clone> Repo<T> {
    pub fn new() -> Self {
        Repo { rows: Vec::new() }
    }

    pub fn add(&mut self, row: T) -> usize {
        self.rows.push(row);
        self.rows.len()
    }

    pub fn get(&self, index: usize) -> Option<T> {
        self.rows.get(index).cloned()
    }

    pub fn find(&self, predicate: impl Fn(&T) -> bool) -> Option<T> {
        self.rows.iter().find(|row| predicate(row)).cloned()
    }
}

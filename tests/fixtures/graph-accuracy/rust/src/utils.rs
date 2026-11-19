pub fn map<T, U>(items: &[T], f: impl Fn(&T) -> U) -> Vec<U> {
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        out.push(f(item));
    }
    out
}

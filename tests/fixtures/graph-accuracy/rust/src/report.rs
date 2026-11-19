use crate::model::{format_name, Order};

pub trait Sink {
    fn write(&mut self, text: &str);
}

pub struct ConsoleSink;

impl Sink for ConsoleSink {
    fn write(&mut self, text: &str) {
        println!("{text}");
    }
}

pub struct MemorySink {
    pub lines: Vec<String>,
}

impl Sink for MemorySink {
    fn write(&mut self, text: &str) {
        self.lines.push(text.to_string());
    }
}

pub fn render(text: &str) -> String {
    text.to_uppercase()
}

pub fn join(cells: &[String]) -> String {
    cells.join(" | ")
}

pub struct Report {
    pub title: String,
}

impl Report {
    pub fn header(&self) -> String {
        render(&self.title)
    }

    pub fn render(&self, rows: &[String]) -> String {
        let mut lines = vec![self.header()];
        lines.extend(rows.iter().map(|row| self.render_row(row)));
        lines.join("\n")
    }

    fn render_row(&self, row: &str) -> String {
        row.to_string()
    }
}

pub struct OrderReport {
    pub report: Report,
    orders: Vec<Order>,
}

impl OrderReport {
    pub fn new(orders: Vec<Order>) -> Self {
        OrderReport {
            report: Report {
                title: "Orders".to_string(),
            },
            orders,
        }
    }

    pub fn render_row(&self, order: &Order) -> String {
        join(&[format_name(&order.user.name), order.total().to_string()])
    }

    pub fn summary(&self) -> String {
        let rows: Vec<String> = self
            .orders
            .iter()
            .map(|order| self.render_row(order))
            .collect();
        self.report.render(&rows)
    }
}

pub fn publish(report: &Report, sink: &mut dyn Sink) {
    sink.write(&report.render(&[]));
}

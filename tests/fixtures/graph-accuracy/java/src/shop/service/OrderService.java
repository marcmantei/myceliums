package shop.service;

import java.util.List;
import java.util.Map;
import shop.config.Config;
import shop.model.Item;
import shop.model.Order;
import shop.model.User;
import shop.store.Repository;

/** Manages orders. */
public class OrderService {
    private final Repository<Order> repo;

    public OrderService(Repository<Order> repo) {
        this.repo = repo;
    }

    public int place(User user, List<Item> items) {
        Order order = new Order(user, items);
        order.save(repo);
        return order.total();
    }

    public String parse(String text) {
        Map<String, String> values = Config.parse(text);
        return values.getOrDefault("currency", "EUR");
    }

    public String currency(String text) {
        return parse(text);
    }
}

package shop.model;

import java.util.ArrayList;
import java.util.List;
import shop.store.Repository;

/** A purchase. */
public class Order {
    public final User user;
    private final List<Item> items;

    public Order(User user, List<Item> items) {
        this.user = user;
        this.items = new ArrayList<>(items);
    }

    public Order save(Repository<Order> repo) {
        repo.add(this, 1);
        return this;
    }

    public int total() {
        return items.stream().map(Item::price).reduce(0, Integer::sum);
    }

    public void add(Item item) {
        items.add(item);
    }
}

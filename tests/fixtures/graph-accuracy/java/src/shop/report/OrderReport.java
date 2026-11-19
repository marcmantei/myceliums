package shop.report;

import java.util.List;
import shop.model.Names;
import shop.model.Order;

/** A report over orders. */
public class OrderReport extends Report {
    private final List<Order> orders;

    public OrderReport(List<Order> orders) {
        super("Orders");
        this.orders = orders;
    }

    @Override
    protected String renderRow(Object row) {
        Order order = (Order) row;
        return Formatting.join(Names.formatName(order.user.name), String.valueOf(order.total()));
    }

    public String summary() {
        return render(orders);
    }

    public static String empty() {
        return new OrderReport(List.of()).summary();
    }
}

package shop;

import java.util.List;
import shop.config.Config;
import shop.model.Item;
import shop.model.Names;
import shop.model.Order;
import shop.model.User;
import shop.report.ConsoleSink;
import shop.report.OrderReport;
import shop.report.Report;
import shop.service.OrderService;
import shop.service.UserService;
import shop.store.Repository;

/** Wires the services together. */
public class Main {
    static String handler(List<String> event) {
        return "handled " + event.size();
    }

    static class Worker {
        String handler(List<String> event) {
            return "worked " + event.size();
        }

        String run(List<String> event) {
            return handler(event);
        }
    }

    public static void main(String[] args) {
        System.out.println(run(List.of(args)));
    }

    static String run(List<String> argv) {
        UserService users = new UserService(new Repository<User>());
        OrderService orders = new OrderService(new Repository<Order>());
        User user = users.register(argv.get(0), argv.get(1));
        orders.place(user, List.of(new Item(3)));
        System.out.println(Names.formatName(user.name));
        new Worker().run(argv);
        handler(argv);
        Report.publish(new OrderReport(List.of()), new ConsoleSink());
        return new Config(java.util.Map.of()).get("argv", "");
    }
}

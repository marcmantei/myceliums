package shop.store;

import java.util.ArrayList;
import java.util.List;
import java.util.Optional;
import java.util.function.Predicate;

/** Rows kept in memory. */
public class Repository<T> {
    private final List<T> rows = new ArrayList<>();

    public int add(T row) {
        rows.add(row);
        return rows.size();
    }

    public int add(T row, int copies) {
        for (int i = 1; i < copies; i++) {
            add(row);
        }
        return add(row);
    }

    public T get(int index) {
        return rows.get(index);
    }

    public Optional<T> find(Predicate<T> predicate) {
        return rows.stream().filter(predicate).findFirst();
    }

    public List<T> all() {
        return new ArrayList<>(rows);
    }
}

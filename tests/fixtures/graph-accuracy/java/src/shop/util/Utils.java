package shop.util;

import java.util.ArrayList;
import java.util.List;
import java.util.function.Function;

/** Collection helpers. */
public final class Utils {
    private Utils() {}

    public static <T, U> List<U> map(List<T> items, Function<T, U> f) {
        List<U> out = new ArrayList<>();
        for (T item : items) {
            out.add(f.apply(item));
        }
        return out;
    }
}

package shop.report;

/** Text helpers. */
public final class Formatting {
    private Formatting() {}

    public static String render(String text) {
        return text.toUpperCase();
    }

    public static String join(String... cells) {
        return String.join(" | ", cells);
    }
}

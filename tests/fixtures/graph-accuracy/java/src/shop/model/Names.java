package shop.model;

/** Display-name formatting. */
public final class Names {
    private Names() {}

    public static String formatName(String name) {
        return name.trim().toUpperCase();
    }
}

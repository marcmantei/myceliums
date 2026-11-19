package shop.validation;

/** Stateless checks. */
public final class Validation {
    private Validation() {}

    public static boolean validate(String[] parts) {
        return parts.length == 2 && !parts[0].isEmpty() && !parts[1].isEmpty();
    }

    public static boolean isEmail(String value) {
        return value.contains("@");
    }
}

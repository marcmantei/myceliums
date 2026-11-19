package shop.validation;

import java.util.List;
import java.util.function.Predicate;

/** Applies a list of rules. */
public class Validator {
    private final List<Predicate<String>> rules;

    public Validator(List<Predicate<String>> rules) {
        this.rules = rules;
    }

    public boolean validate(String value) {
        return rules.stream().allMatch(rule -> check(rule, value));
    }

    boolean check(Predicate<String> rule, String value) {
        return rule.test(value);
    }
}

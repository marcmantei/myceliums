package shop.service;

import static shop.auth.Auth.parse;

import java.util.List;
import shop.auth.Auth;
import shop.auth.Token;
import shop.config.Config;
import shop.model.User;
import shop.store.Repository;
import shop.util.Utils;
import shop.validation.Validation;
import shop.validation.Validator;

/** Manages users. */
public class UserService {
    private final Repository<User> repo;
    private final Validator validator;

    public UserService(Repository<User> repo) {
        this.repo = repo;
        this.validator = new Validator(List.of(Validation::isEmail));
    }

    public User register(String name, String email) {
        if (!validator.validate(email)) {
            throw new IllegalArgumentException("invalid email");
        }
        User user = new User(name, email);
        return user.save(repo);
    }

    public Token login(String raw) {
        Token token = parse(raw);
        return Auth.refresh(token, 0);
    }

    public String settings(String text) {
        Config cfg = Config.load(text);
        return cfg.get("theme", "light");
    }

    public User get(int index) {
        return repo.get(index);
    }

    public List<String> names() {
        return Utils.map(repo.all(), user -> user.displayName());
    }
}

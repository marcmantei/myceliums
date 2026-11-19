package shop.model;

import shop.store.Repository;

/** A customer. */
public class User {
    public final String name;
    public final String email;

    public User(String name, String email) {
        this.name = name;
        this.email = email;
    }

    public User save(Repository<User> repo) {
        repo.add(this);
        return this;
    }

    public String displayName() {
        return Names.formatName(name);
    }
}

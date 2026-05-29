package com.example.app;

public class UserService {
    private String name;
    private int maxRetries = 3;

    public UserService(String name) {
        this.name = name;
    }

    public String getUser(String id) {
        return findById(id);
    }

    public void deleteUser(String id) {
        System.out.println("Deleting user: " + id);
    }

    private String findById(String id) {
        return "User-" + id;
    }
}

enum Status {
    ACTIVE,
    INACTIVE,
    ERROR
}

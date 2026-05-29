// User service with CommonJS and ESM exports
export class UserService {
    constructor() {
        this.users = new Map();
    }

    getUser(id) {
        return this.users.get(id);
    }

    addUser(id, user) {
        this.users.set(id, user);
        return this.validateUser(user);
    }

    deleteUser(id) {
        return this.users.delete(id);
    }

    validateUser(user) {
        if (!user.name) throw new Error('User must have a name');
        return true;
    }
}

export default UserService;

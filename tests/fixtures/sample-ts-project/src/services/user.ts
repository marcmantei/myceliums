import { Database } from '../db';

export interface User {
    id: string;
    name: string;
    email: string;
}

export class UserService {
    private db: Database;

    constructor() {
        this.db = new Database();
    }

    getUser(id: string): User | null {
        return this.db.findById('users', id);
    }

    createUser(data: Partial<User>): User {
        const user = { ...data, id: generateId() } as User;
        this.db.insert('users', user);
        return user;
    }

    deleteUser(id: string): boolean {
        return this.db.delete('users', id);
    }

    listUsers(): User[] {
        return this.db.findAll('users');
    }
}

function generateId(): string {
    return Math.random().toString(36).substring(2);
}

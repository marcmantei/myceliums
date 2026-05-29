export class Database {
    private store: Map<string, Map<string, any>> = new Map();

    findById(collection: string, id: string): any {
        return this.store.get(collection)?.get(id) ?? null;
    }

    findAll(collection: string): any[] {
        const coll = this.store.get(collection);
        return coll ? Array.from(coll.values()) : [];
    }

    insert(collection: string, record: any): void {
        if (!this.store.has(collection)) {
            this.store.set(collection, new Map());
        }
        this.store.get(collection)!.set(record.id, record);
    }

    delete(collection: string, id: string): boolean {
        return this.store.get(collection)?.delete(id) ?? false;
    }
}

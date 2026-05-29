import Foundation

// Swift actors for concurrency safety
actor BankAccount {
    private var balance: Double = 0.0

    func deposit(amount: Double) {
        balance += amount
    }

    func withdraw(amount: Double) -> Bool {
        guard balance >= amount else { return false }
        balance -= amount
        return true
    }

    func getBalance() -> Double {
        return balance
    }
}

// Generic struct
struct Stack<T> {
    private var items: [T] = []

    mutating func push(_ item: T) {
        items.append(item)
    }

    mutating func pop() -> T? {
        return items.popLast()
    }
}

// Protocol with associated type
protocol Identifiable {
    associatedtype ID
    var id: ID { get }
}

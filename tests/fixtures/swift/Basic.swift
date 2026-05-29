import Foundation
import UIKit

protocol Repository {
    func findById(id: String) -> Any?
    func save(entity: Any) -> Any
    func deleteById(id: String)
}

struct User {
    let id: String
    var name: String
    var email: String
}

class UserService {
    var repository: Repository

    init(repository: Repository) {
        self.repository = repository
    }

    func getUser(id: String) -> User? {
        return repository.findById(id: id) as? User
    }

    func createUser(name: String, email: String) -> User {
        let user = User(id: UUID().uuidString, name: name, email: email)
        _ = repository.save(entity: user)
        return user
    }

    func deleteUser(id: String) {
        repository.deleteById(id: id)
    }
}

enum UserRole {
    case admin
    case user
    case guest
}

typealias UserID = String

extension UserService {
    func getAllUsers() -> [User] {
        return []
    }
}

func main() {
    let service = UserService(repository: InMemoryRepository())
    let user = service.createUser(name: "Alice", email: "alice@example.com")
    print(user.name)
}

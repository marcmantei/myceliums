package com.example.myapp

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.map
import java.util.UUID as JavaUUID

data class User(val id: String, val name: String, val email: String)

sealed class Result<out T> {
    data class Success<T>(val data: T) : Result<T>()
    data class Error(val message: String) : Result<Nothing>()
}

interface Repository<T> {
    fun findById(id: String): T?
    fun save(entity: T): T
    fun deleteById(id: String)
}

object AppConfig {
    val version = "1.0.0"

    fun getEnvironment(): String {
        return System.getenv("ENV") ?: "development"
    }
}

class UserService(private val repository: Repository<User>) {
    companion object {
        fun create(repository: Repository<User>): UserService {
            return UserService(repository)
        }
    }

    fun getUser(id: String): Result<User> {
        val user = repository.findById(id)
        return if (user != null) {
            Result.Success(user)
        } else {
            Result.Error("User not found")
        }
    }

    fun createUser(name: String, email: String): Result<User> {
        val user = User(
            id = JavaUUID.randomUUID().toString(),
            name = name,
            email = email
        )
        val saved = repository.save(user)
        return Result.Success(saved)
    }

    fun deleteUser(id: String) {
        repository.deleteById(id)
    }
}

enum class UserRole {
    ADMIN,
    USER,
    GUEST
}

fun main() {
    val config = AppConfig
    println(config.getEnvironment())
    println(AppConfig.version)
}

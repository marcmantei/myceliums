#include <iostream>
#include <string>
#include <vector>

namespace myapp {

enum class Color {
    Red,
    Green,
    Blue
};

class Animal {
public:
    Animal(const std::string& name) : name_(name) {}
    virtual ~Animal() {}

    virtual std::string speak() const = 0;
    std::string getName() const { return name_; }

protected:
    std::string name_;
};

class Dog : public Animal {
public:
    Dog(const std::string& name) : Animal(name) {}

    std::string speak() const override {
        return "Woof!";
    }

    void fetch(const std::string& item) {
        std::cout << name_ << " fetches " << item << std::endl;
    }
};

struct Point {
    double x;
    double y;

    double distance() const {
        return std::sqrt(x * x + y * y);
    }
};

template<typename T>
T max_value(T a, T b) {
    return (a > b) ? a : b;
}

template<typename T>
class Container {
public:
    void add(const T& item) {
        items_.push_back(item);
    }

    size_t size() const {
        return items_.size();
    }

private:
    std::vector<T> items_;
};

typedef unsigned long ulong;
using StringVec = std::vector<std::string>;

void greet(const std::string& name) {
    std::cout << "Hello, " << name << "!" << std::endl;
}

int main() {
    Dog dog("Rex");
    dog.fetch("ball");
    greet("world");
    auto result = max_value(3, 5);

    Container<int> c;
    c.add(42);

    auto p = new Point{1.0, 2.0};
    delete p;

    return 0;
}

} // namespace myapp

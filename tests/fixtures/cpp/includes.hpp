#ifndef INCLUDES_HPP
#define INCLUDES_HPP

#include <string>
#include <memory>
#include "basic.cpp"

namespace mylib {

class Service {
public:
    Service() = default;
    virtual ~Service() = default;

    virtual void start() = 0;
    virtual void stop() = 0;
};

class HttpService : public Service {
public:
    HttpService(int port);

    void start() override;
    void stop() override;

    int getPort() const;

private:
    int port_;
    bool running_;
};

template<typename T>
class Repository {
public:
    virtual T findById(int id) = 0;
    virtual void save(const T& entity) = 0;
};

enum class HttpMethod {
    GET,
    POST,
    PUT,
    DELETE
};

using ServicePtr = std::shared_ptr<Service>;

} // namespace mylib

#endif // INCLUDES_HPP

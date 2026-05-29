<?php

namespace App\Models;

use App\Contracts\Renderable;
use App\Services\Logger as AppLogger;

const MAX_RETRIES = 3;

interface Cacheable {
    public function cacheKey(): string;
}

trait Timestamps {
    public function createdAt(): string {
        return $this->created_at;
    }
}

enum Status: string {
    case Active = 'active';
    case Inactive = 'inactive';
}

class User implements Cacheable {
    use Timestamps;

    public string $name;
    private int $age;

    public function __construct(string $name, int $age) {
        $this->name = $name;
        $this->age = $age;
    }

    public function greet(): string {
        $logger = new AppLogger();
        $logger->info("Greeting user");
        return sprintf("Hello, %s", $this->name);
    }

    public function cacheKey(): string {
        return "user:" . $this->name;
    }

    public static function create(string $name, int $age): self {
        return new self($name, $age);
    }
}

function findUser(string $name): ?User {
    $user = User::create($name, 0);
    $user->greet();
    return $user;
}

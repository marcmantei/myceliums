<?php

namespace App\Traits;

use App\Contracts\Loggable;
use App\Models\User;

trait HasTimestamps {
    private ?DateTime $createdAt = null;
    private ?DateTime $updatedAt = null;

    public function touch(): void {
        $this->updatedAt = new DateTime();
    }

    public function getCreatedAt(): ?DateTime {
        return $this->createdAt;
    }
}

interface Serializable {
    public function serialize(): string;
    public function deserialize(string $data): static;
}

interface Cacheable extends Serializable {
    public function getCacheKey(): string;
    public function getTtl(): int;
}

class Repository implements Cacheable {
    use HasTimestamps;

    private array $items = [];

    public function find(int $id): ?User {
        return $this->items[$id] ?? null;
    }

    public function save(User $user): void {
        $this->items[$user->getId()] = $user;
        $this->touch();
    }

    public function serialize(): string {
        return json_encode($this->items);
    }

    public function deserialize(string $data): static {
        $this->items = json_decode($data, true);
        return $this;
    }

    public function getCacheKey(): string {
        return 'repository:' . static::class;
    }

    public function getTtl(): int {
        return 3600;
    }
}

function buildRepository(string $class): Repository {
    return new $class();
}

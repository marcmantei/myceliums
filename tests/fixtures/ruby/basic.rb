require 'json'
require_relative 'helpers/utils'

MAX_RETRIES = 3

module Serializable
  def to_json
    JSON.generate(attributes)
  end
end

class User
  include Serializable

  attr_reader :name, :email

  def initialize(name, email)
    @name = name
    @email = email
  end

  def greet
    puts "Hello, #{@name}!"
  end

  def self.create(name, email)
    new(name, email)
  end
end

class Admin < User
  def initialize(name, email)
    super(name, email)
  end

  def admin_action
    yield if block_given?
  end
end

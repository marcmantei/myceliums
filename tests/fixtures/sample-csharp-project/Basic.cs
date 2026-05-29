using System;
using System.Collections.Generic;

namespace MyApp.Models
{
    public class UserService
    {
        public string Name { get; set; }
        public int Count { get; private set; }

        public UserService(string name)
        {
            Name = name;
            Count = 0;
        }

        public string GetUser(string id)
        {
            var result = ProcessId(id);
            Console.WriteLine(result);
            return result;
        }

        public void DeleteUser(string id)
        {
            var svc = new LogService();
            svc.Log("Deleted " + id);
        }

        private string ProcessId(string id)
        {
            return id.Trim();
        }
    }

    public struct Point
    {
        public int X { get; set; }
        public int Y { get; set; }
    }

    public enum Status
    {
        Active,
        Inactive,
        Error
    }

    public delegate void EventHandler(object sender, EventArgs e);

    public record Person(string Name, int Age);
}

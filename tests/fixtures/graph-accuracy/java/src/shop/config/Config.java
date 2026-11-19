package shop.config;

import java.util.HashMap;
import java.util.Map;

/** Settings loaded from `key=value;...` text. */
public class Config {
    private final Map<String, String> values;

    public Config(Map<String, String> values) {
        this.values = values;
    }

    public static Map<String, String> parse(String text) {
        Map<String, String> data = new HashMap<>();
        for (String pair : text.split(";")) {
            String[] kv = pair.split("=", 2);
            data.put(kv[0], kv.length > 1 ? kv[1] : "");
        }
        return normalize(data);
    }

    static Map<String, String> normalize(Map<String, String> data) {
        Map<String, String> out = new HashMap<>();
        data.forEach((key, value) -> out.put(key.toLowerCase(), value));
        return out;
    }

    public static Config load(String text) {
        return new Config(parse(text));
    }

    public String get(String key, String fallback) {
        String value = values.get(key);
        return value == null ? fallback : value;
    }
}

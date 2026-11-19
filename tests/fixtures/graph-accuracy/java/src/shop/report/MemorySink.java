package shop.report;

import java.util.ArrayList;
import java.util.List;

/** Collects lines. */
public class MemorySink implements Sink {
    public final List<String> lines = new ArrayList<>();

    @Override
    public void write(String text) {
        lines.add(text);
    }
}

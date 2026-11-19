package shop.report;

import java.util.ArrayList;
import java.util.List;

/** A titled list of rows. */
public class Report {
    protected final String title;

    public Report(String title) {
        this.title = title;
    }

    public String header() {
        return Formatting.render(title);
    }

    public String render(List<?> rows) {
        List<String> lines = new ArrayList<>();
        lines.add(header());
        for (Object row : rows) {
            lines.add(renderRow(row));
        }
        return String.join("\n", lines);
    }

    protected String renderRow(Object row) {
        return String.valueOf(row);
    }

    public static void publish(Report report, Sink sink) {
        sink.write(report.render(List.of()));
    }
}

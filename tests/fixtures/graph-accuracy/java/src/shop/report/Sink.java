package shop.report;

/** Receives rendered text. */
public interface Sink {
    void write(String text);
}

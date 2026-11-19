package shop.report;

/** Prints. */
public class ConsoleSink implements Sink {
    @Override
    public void write(String text) {
        System.out.println(text);
    }
}

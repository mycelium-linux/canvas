package dev.fpl.canvas;

public final class PageRotationTest {
    private static void expect(boolean ok) { if (!ok) throw new AssertionError(); }
    public static void main(String[] args) {
        PageRotation r = new PageRotation();
        expect(r.enter("lathe", 0, 5000));
        expect(r.due(4999, 5000, 2, 0) == -1);
        expect(r.due(5000, 5000, 2, 0) == 1);
        expect(r.due(10000, 5000, 2, 1) == 2);
        expect(r.due(15000, 5000, 2, 2) == 0);
        expect(r.due(30000, 0, 2, 0) == -1);
        expect(r.due(30000, 4999, 2, 0) == -1);
        expect(r.due(30000, 300001, 2, 0) == -1);
        expect(r.due(30000, 5000, 0, 0) == -1);
        expect(!r.enter("lathe", 16000, 5000));
        r.defer(19000, 5000);
        expect(r.due(23999, 5000, 2, 0) == -1);
        expect(r.due(24000, 5000, 2, 0) == 1);
        expect(r.enter("other", 25000, 5000));
        expect(r.due(29999, 5000, 2, 1) == -1);
        expect(r.due(30000, 5000, 2, 1) == 2);
        System.out.println("Page rotation boundary, wrap, disable, manual defer and workspace reset passed");
    }
}

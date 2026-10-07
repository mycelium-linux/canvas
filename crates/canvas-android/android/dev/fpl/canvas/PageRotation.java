package dev.fpl.canvas;

/** Monotonic presentation timing only. The shared endpoint owns page state. */
final class PageRotation {
    private String workspace = "";
    private long next;
    boolean enter(String id, long now, int interval) {
        if (workspace.equals(id)) return false;
        workspace = id;
        defer(now, interval);
        return true;
    }
    void defer(long now, int interval) { next = now + Math.max(5000, interval); }
    int due(long now, int interval, int additionalPages, int current) {
        if (interval < 5000 || interval > 300000 || additionalPages < 1 || now < next) return -1;
        defer(now, interval);
        return (current + 1) % (additionalPages + 1);
    }
}

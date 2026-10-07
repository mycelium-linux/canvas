package dev.fpl.canvas;

/** Bounded exact-state detector for the pinned 64x64 Life view. */
final class LifeSettlement {
    private final String[] history = new String[32];
    private long generation = -1;
    private int cursor, count, repeats;
    void reset() {
        java.util.Arrays.fill(history,null);
        generation=-1; cursor=0; count=0; repeats=0;
    }
    boolean observe(long next, String state) {
        if(next<0 || state.length()!=4096 || !state.matches("[01]+")) throw new IllegalArgumentException("Invalid Life observation");
        if(next==generation) return false; // A manual pause is not settlement.
        if(next<generation) reset();
        generation=next;
        boolean seen=false;
        for(int i=0;i<count;i++) if(state.equals(history[i])) {seen=true;break;}
        repeats=seen?repeats+1:0;
        history[cursor]=state; cursor=(cursor+1)%history.length; count=Math.min(count+1,history.length);
        return repeats>=16;
    }
}

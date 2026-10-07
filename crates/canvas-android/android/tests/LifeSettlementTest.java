package dev.fpl.canvas;
public final class LifeSettlementTest {
    private static String state(int n) {
        char[] value=new char[4096]; java.util.Arrays.fill(value,'0');
        if(n>=0)value[n]='1'; return new String(value);
    }
    private static void require(boolean condition) { if(!condition)throw new AssertionError(); }
    public static void main(String[] args) {
        LifeSettlement detector=new LifeSettlement();
        for(int i=0;i<16;i++)require(!detector.observe(i,state(-1)));
        require(detector.observe(16,state(-1))); // Empty/still life.
        detector.reset();
        for(int i=0;i<17;i++)require(!detector.observe(i,state(i%2)));
        require(detector.observe(17,state(1))); // Period-two oscillator.
        detector.reset();
        for(int i=0;i<100;i++)require(!detector.observe(0,state(1))); // Manual pause.
        for(int i=1;i<100;i++)require(!detector.observe(i,state(i))); // Changing world.
        require(!detector.observe(0,state(1))); // Reset generation clears history.
        try{detector.observe(1,"bad");throw new AssertionError();}catch(IllegalArgumentException expected){}
        System.out.println("Life settlement: empty, oscillator, pause, evolution, reset, bounds passed");
    }
}

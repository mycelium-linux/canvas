package dev.fpl.canvas;

public final class PdfViewportTest {
    static void require(boolean value) { if(!value)throw new AssertionError(); }
    static boolean near(float a,float b) { return Math.abs(a-b)<0.01f; }
    public static void main(String[] args) {
        PdfViewport v=new PdfViewport();v.size(1920,1000,1800,2560);
        require(v.scale()*v.pageHeight<=1000.001f);
        v.zoom();require(near(v.scale()*v.pageWidth,1920));
        v.zoom();require(near(v.scale()*v.pageWidth,3840));
        v.pan(-100000,100000);require(v.x==-v.limitX()&&v.y==v.limitY());
        v.zoom();require(near(v.scale()*v.pageWidth,7680));
        v.zoom();require(v.level==0&&v.x==0&&v.y==0);
        v.size(0,0,0,0);require(Float.isFinite(v.scale()));
        v.size(1000,1900,1800,2560);require(near(v.limitX(),0)&&near(v.limitY(),0));
        v.size(1920,1000,1800,2560);
        v.drawing(new double[]{0.25,0.10,0.5,0.3});v.fitDrawing();
        require(near(v.centerX(),900)&&near(v.centerY(),640));
        require(v.pageWidth*v.bw*v.scale()<=1920 && v.pageHeight*v.bh*v.scale()<=1000);
        require(near(v.limitX(),0)&&near(v.limitY(),0));
        float drawingScale=v.scale();v.zoom();require(near(v.scale(),drawingScale*2));
        v.pan(100000,-100000);require(near(v.x,v.limitX())&&near(v.y,-v.limitY()));
        v.fit();require(near(v.centerY(),1280)&&v.level==0);
        System.out.println("PDF viewport fit/read/2x/4x/pan/reset bounds passed");
    }
}

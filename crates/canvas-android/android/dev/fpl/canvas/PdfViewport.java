package dev.fpl.canvas;

/** Bounded local presentation state; coordinates are viewport pixels. */
final class PdfViewport {
    int width, height, pageWidth, pageHeight, level;
    float x, y;
    float bx, by, bw=1, bh=1;
    boolean hasDrawing;
    void drawing(double[] region) {
        hasDrawing=region!=null;
        bx=hasDrawing?(float)region[0]:0;by=hasDrawing?(float)region[1]:0;
        bw=hasDrawing?(float)region[2]:1;bh=hasDrawing?(float)region[3]:1;
        fit();
    }
    void size(int w, int h, int pw, int ph) {
        width=Math.max(1,w); height=Math.max(1,h);
        pageWidth=Math.max(1,pw); pageHeight=Math.max(1,ph); clamp();
    }
    float scale() {
        float fit=Math.min((float)width/pageWidth,(float)height/pageHeight);
        float content=hasDrawing?Math.min(width/(pageWidth*bw),height/(pageHeight*bh))*0.92f:(float)width/pageWidth;
        return level==0?fit:content*(level==1?1:level==2?2:4);
    }
    void zoom() { level=(level+1)%4; x=0; y=0; clamp(); }
    void fit() { level=0; x=0; y=0; }
    void fitDrawing() { level=1; x=0; y=0; clamp(); }
    void pan(float dx, float dy) { x+=dx; y+=dy; clamp(); }
    private void clamp() {
        x=Math.max(-limitX(),Math.min(limitX(),x));
        y=Math.max(-limitY(),Math.min(limitY(),y));
    }
    float centerX() { return pageWidth*(level>0&&hasDrawing?bx+bw/2:0.5f); }
    float centerY() { return pageHeight*(level>0&&hasDrawing?by+bh/2:0.5f); }
    float limitX() { return Math.max(0,(pageWidth*(level>0&&hasDrawing?bw:1)*scale()-width)/2); }
    float limitY() { return Math.max(0,(pageHeight*(level>0&&hasDrawing?bh:1)*scale()-height)/2); }
    String mode() { return level==0?"Sheet":level==1?(hasDrawing?"Drawing":"Read"):level==2?"2x":"4x"; }
}

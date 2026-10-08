package dev.fpl.canvas;

import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.view.KeyEvent;
import android.view.View;

/** One bounded cached raster; zoom/pan never allocate or decode another page. */
final class PdfPageView extends View {
    private final PdfViewport viewport=new PdfViewport();
    private final Paint paint=new Paint(Paint.ANTI_ALIAS_FLAG|Paint.FILTER_BITMAP_FLAG);
    private Bitmap page;
    private String error="";
    private final Runnable changed;
    private final int backgroundColor, textColor;
    PdfPageView(Context context,int backgroundColor,int textColor,Runnable changed) {
        super(context); this.changed=changed; setFocusable(true);
        this.backgroundColor=backgroundColor;this.textColor=textColor;
        setFocusableInTouchMode(true);
    }
    void page(Bitmap value,double[] bounds) { page=value; error=""; viewport.drawing(bounds); resize(); invalidate(); }
    void error(String message) { error=message; invalidate(); }
    void clear() { page=null; invalidate(); }
    void zoom() { viewport.zoom(); changed.run(); invalidate(); }
    void fit() { viewport.fit(); changed.run(); invalidate(); }
    void fitDrawing() { viewport.fitDrawing(); changed.run(); invalidate(); }
    String mode() { return viewport.mode(); }
    boolean enlarged() { return viewport.level>0; }
    private void resize() {
        if(page!=null)viewport.size(getWidth(),getHeight(),page.getWidth(),page.getHeight());
    }
    @Override protected void onSizeChanged(int w,int h,int oldw,int oldh) { resize(); }
    @Override protected void onDraw(Canvas canvas) {
        if(!error.isEmpty()) {
            canvas.drawColor(backgroundColor);
            paint.setColor(textColor);paint.setTextSize(28*getResources().getDisplayMetrics().scaledDensity);
            float y=getHeight()/2f;
            for(String line:new String[]{"PDF sheet unavailable",error,"OK Retry · BACK return · MENU workspaces"}) {
                // Keep diagnostics inside the viewport even for long owner errors.
                float size=paint.getTextSize();
                float available=Math.max(1,getWidth()-64);
                if(paint.measureText(line)>available)paint.setTextSize(size*available/paint.measureText(line));
                canvas.drawText(line,(getWidth()-paint.measureText(line))/2,y,paint);
                paint.setTextSize(size);y+=size*1.7f;
            }
            return;
        }
        if(page==null)return;
        float scale=viewport.scale();
        canvas.save();
        canvas.translate(getWidth()/2f-viewport.centerX()*scale+viewport.x,
            getHeight()/2f-viewport.centerY()*scale+viewport.y);
        canvas.scale(scale,scale); canvas.drawBitmap(page,0,0,paint); canvas.restore();
    }
    @Override public boolean onKeyDown(int key,KeyEvent event) {
        if(key==KeyEvent.KEYCODE_DPAD_CENTER||key==KeyEvent.KEYCODE_ENTER) {
            if(event.getRepeatCount()==0) {
                if(error.isEmpty())zoom();else changed.run();
            }
            return true;
        }
        if(enlarged() && (viewport.limitX()>0 || viewport.limitY()>0)) {
            float step=Math.max(24,Math.min(getWidth(),getHeight())*0.12f);
            switch(key) {
                case KeyEvent.KEYCODE_DPAD_LEFT:viewport.pan(step,0);break;
                case KeyEvent.KEYCODE_DPAD_RIGHT:viewport.pan(-step,0);break;
                case KeyEvent.KEYCODE_DPAD_UP:viewport.pan(0,step);break;
                case KeyEvent.KEYCODE_DPAD_DOWN:viewport.pan(0,-step);break;
                default:return super.onKeyDown(key,event);
            }
            invalidate();return true;
        }
        return super.onKeyDown(key,event);
    }
}

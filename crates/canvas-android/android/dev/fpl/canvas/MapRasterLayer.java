package dev.fpl.canvas;

import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.Canvas;
import android.graphics.Paint;
import org.json.JSONArray;
import org.json.JSONObject;
import java.util.ArrayList;
import java.util.HashMap;

/** Bounded asynchronous asset decoding; geometry and identity checks are Rust-owned. */
final class MapRasterLayer {
    private static final class Image { Bitmap bitmap; float[] vertices; int rows; }
    private static final class State {
        Bitmap composite;
        float[] plot; String attribution = ""; String error; boolean complete;
    }
    private volatile State state = new State();
    private volatile boolean loading;
    private long requested;
    private String requestedKey = "";
    private final HashMap<String,Bitmap> cache = new HashMap<>();
    private final Paint paint = new Paint(Paint.FILTER_BITMAP_FLAG);

    void request(final JSONObject value, final int width, final int height) {
        long now = android.os.SystemClock.elapsedRealtime();
        if (loading || now-requested<1000) return;
        try {
            String key=width+":"+height+":"+value.getJSONObject("extent")+":"+value.optJSONObject("basemap");
            if(key.equals(requestedKey)&&state.complete)return;
            requestedKey=key;
        } catch(Exception e) { android.util.Log.e("CanvasTiles",e.toString()); return; }
        final String map=value.toString();
        loading=true; requested=now;
        new Thread(() -> {
            State next=new State();
            try {
                JSONObject plan=new JSONObject(TvActivity.nativeMapRaster(map,width,height));
                if(plan.has("error")) throw new IllegalStateException(plan.getString("error"));
                JSONArray plot=plan.optJSONArray("plot");
                if(plot!=null) {
                    next.plot=new float[4]; for(int i=0;i<4;i++)next.plot[i]=(float)plot.getDouble(i);
                    next.attribution=plan.getString("attribution");
                }
                JSONArray tiles=plan.getJSONArray("tiles");
                next.complete=tiles.length()==plan.optInt("expected",0);
                if(tiles.length()>64)throw new IllegalStateException("Tile budget exceeded");
                Bitmap composite=Bitmap.createBitmap(width,height,Bitmap.Config.ARGB_8888);
                Canvas composed=new Canvas(composite);
                Paint bitmapPaint=new Paint(Paint.FILTER_BITMAP_FLAG);
                if(next.plot!=null)composed.clipRect(next.plot[0],next.plot[1],next.plot[2],next.plot[3]);
                for(int i=0;i<tiles.length();i++) {
                    JSONObject tile=tiles.getJSONObject(i); String sha=tile.getString("sha256");
                    Bitmap bitmap=cache.get(sha);
                    if(bitmap==null) {
                        if(cache.size()>=64)throw new IllegalStateException("Decode cache budget exceeded");
                        BitmapFactory.Options bounds=new BitmapFactory.Options(); bounds.inJustDecodeBounds=true;
                        BitmapFactory.decodeFile(tile.getString("path"),bounds);
                        if(bounds.outWidth!=256||bounds.outHeight!=256)throw new IllegalStateException("Invalid raster dimensions");
                        bitmap=BitmapFactory.decodeFile(tile.getString("path"));
                        if(bitmap==null)throw new IllegalStateException("Raster decode failed");
                        cache.put(sha,bitmap);
                    }
                    Image image=new Image(); image.bitmap=bitmap; image.rows=tile.getInt("rows");
                    JSONArray vertices=tile.getJSONArray("vertices");
                    image.vertices=new float[vertices.length()*2];
                    for(int v=0;v<vertices.length();v++) {image.vertices[v*2]=(float)vertices.getJSONArray(v).getDouble(0);image.vertices[v*2+1]=(float)vertices.getJSONArray(v).getDouble(1);}
                    composed.drawBitmapMesh(image.bitmap,1,image.rows,image.vertices,0,null,0,bitmapPaint);
                }
                next.composite=composite;
                android.util.Log.i("CanvasTiles","loaded="+tiles.length()+" expected="+plan.optInt("expected"));
            } catch(Exception e) { next.error=e.getMessage(); android.util.Log.e("CanvasTiles",next.error); }
            if(next.error==null)state=next;
            loading=false;
        },"CanvasRasterDecode").start();
    }
    void draw(Canvas target) {
        State ready=state;
        if(ready.plot==null)return;
        // One retained GPU bitmap per frame, not 20 changing texture meshes.
        if(ready.composite!=null)target.drawBitmap(ready.composite,0,0,paint);
    }
    void attribution(Canvas target) {
        State ready=state; if(ready.plot==null)return;
        paint.setTextSize(11); paint.setColor(0xff10191e);
        float x=ready.plot[2]-paint.measureText(ready.attribution)-8, y=ready.plot[3]-5;
        paint.setColor(0xddffffff); target.drawRect(x-4,y-13,ready.plot[2],y+3,paint);
        paint.setColor(0xff10191e); target.drawText(ready.attribution,x,y,paint);
    }
}

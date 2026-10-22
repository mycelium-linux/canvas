package dev.fpl.canvas;

import org.json.JSONObject;
import org.json.JSONArray;
import java.util.concurrent.ThreadPoolExecutor;
import java.util.concurrent.ArrayBlockingQueue;
import java.util.concurrent.TimeUnit;

/** One in-flight render revision. The UI never waits for JSON or Rust geometry. */
final class MapFramePump {
    static final class Ready {
        final JSONObject snapshot;
        final float[] records;
        final JSONArray labels;
        final String raw;
        final int width,height;
        Ready(JSONObject snapshot,float[] records,JSONArray labels,String raw,int width,int height) {
            this.snapshot=snapshot; this.records=records; this.labels=labels;
            this.raw=raw;this.width=width;this.height=height;
        }
    }
    private final String id, appearance;
    private volatile Ready ready;
    private volatile boolean pending;
    private volatile String error;
    private long kioskTime;
    private final ThreadPoolExecutor worker = new ThreadPoolExecutor(1,1,10,TimeUnit.SECONDS,
        new ArrayBlockingQueue<Runnable>(1));
    MapFramePump(String id,String appearance) {this.id=id;this.appearance=appearance;worker.allowCoreThreadTimeOut(true);}
    Ready request(final int width,final int height) {
        if(!pending) {
            pending=true;
            worker.execute(() -> {
                try {
                    long now=android.os.SystemClock.elapsedRealtime();
                    if(now-kioskTime>=250) {
                        kioskTime=now;
                        JSONObject kiosk=new JSONObject(TvActivity.nativeMapKiosk(id));
                        if(kiosk.has("error"))throw new IllegalStateException(kiosk.getString("error"));
                        if(kiosk.optBoolean("advanced"))android.util.Log.i("CanvasKiosk","selected="+kiosk.optString("selected"));
                    }
                    JSONObject snapshot=new JSONObject(TvActivity.nativeSignalSnapshot(id));
                    if(snapshot.has("error"))throw new IllegalStateException(snapshot.getString("error"));
                    String raw=snapshot.getJSONObject("frame").toString();
                    float[] records=TvActivity.nativeSignalFrame(width,height,raw,appearance);
                    JSONArray labels=new JSONArray(TvActivity.nativeSignalLabels(width,height,raw));
                    if(records==null||records.length%8!=0)throw new IllegalStateException("Invalid map geometry");
                    ready=new Ready(snapshot,records,labels,raw,width,height); error=null;
                } catch(Exception e) {error=e.getMessage();android.util.Log.e("CanvasMap",error);}
                finally {pending=false;}
            });
        }
        if(error!=null)android.util.Log.e("CanvasMap","Retaining last good frame: "+error);
        Ready frame=ready;
        return frame!=null&&frame.width==width&&frame.height==height ? frame : null;
    }
    void close(){worker.shutdownNow();}
}

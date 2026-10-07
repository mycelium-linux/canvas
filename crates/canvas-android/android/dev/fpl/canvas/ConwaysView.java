package dev.fpl.canvas;

import android.content.Context;
import android.graphics.Color;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.webkit.WebResourceRequest;
import android.webkit.WebResourceResponse;
import android.widget.FrameLayout;
import android.widget.TextView;
import java.io.ByteArrayInputStream;
import org.json.JSONObject;

/** Offline, pinned original Rust/WASM app. No native bridge or arbitrary URLs. */
final class ConwaysView extends FrameLayout {
    private static final String ORIGIN = "https://canvas.invalid/conways/";
    private final WebView web;
    private final TextView status;
    private boolean ready;
    private boolean disposed, active=true, restarting;
    private final LifeSettlement settlement=new LifeSettlement();
    private final Runnable sample=() -> sampleLife();
    ConwaysView(Context context, JSONObject palette) throws Exception {
        super(context);
        int bg = Color.parseColor(palette.getString("background"));
        setBackgroundColor(bg);
        web = new WebView(context);
        web.setBackgroundColor(bg);
        web.getSettings().setJavaScriptEnabled(true); // Original Rust/WASM bootstrap only.
        web.getSettings().setAllowFileAccess(false);
        web.getSettings().setAllowContentAccess(false);
        web.getSettings().setDomStorageEnabled(false);
        web.getSettings().setMediaPlaybackRequiresUserGesture(true);
        web.setFocusable(false);
        addView(web, new LayoutParams(-1,-1));
        status = new TextView(context);
        status.setText("Loading Conway’s Rust/WASM simulation…");
        status.setTextColor(Color.parseColor(palette.getString("text")));
        status.setTextSize(20);
        addView(status);
        final String css = "html,body{margin:0!important;width:100%;height:100%;overflow:hidden;background:"+palette.getString("background")+"!important;color:"+palette.getString("text")+"!important}body{display:block!important}h1,.container,#input-bar{display:none!important}.game{position:absolute;inset:0;display:flex;align-items:center;justify-content:center;padding:0!important;background:transparent!important}canvas{width:min(94vw,94vh)!important;height:min(94vw,94vh)!important;image-rendering:pixelated}body:after{content:'Conway’s Game of Life · ajmwagar\\A Play/Pause: pause · Fast-forward: random · Rewind: clear\\A B3/S23 · Wrapped edges · Original Rust/WASM';white-space:pre;position:fixed;bottom:1vh;right:2vw;font:14px monospace;color:"+palette.getString("text_muted")+"}";
        web.setWebViewClient(new WebViewClient() {
            @Override public WebResourceResponse shouldInterceptRequest(WebView view, WebResourceRequest request) {
                String url = request.getUrl().toString();
                String name = url.startsWith(ORIGIN) ? url.substring(ORIGIN.length()) : "";
                if (!(name.equals("index.html") || name.equals("bootstrap.js") || name.equals("0.bootstrap.js") || name.equals("b2e30a57dea6e1d2f410.module.wasm") || name.equals("bf459d9768236238e3e4.module.wasm")))
                    return new WebResourceResponse("text/plain","UTF-8",403,"Blocked",null,new ByteArrayInputStream(new byte[0]));
                try {
                    return new WebResourceResponse(name.endsWith(".wasm")?"application/wasm":name.endsWith(".js")?"application/javascript":"text/html", "UTF-8",context.getAssets().open("conways/"+name));
                } catch (Exception error) {
                    post(() -> status.setText("Life asset failed: "+name));
                    return new WebResourceResponse("text/plain","UTF-8",500,"Missing asset",null,new ByteArrayInputStream(new byte[0]));
                }
            }
            @Override public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest request) { return true; }
            @Override public void onPageFinished(WebView view, String url) {
                view.evaluateJavascript("var s=document.createElement('style');s.textContent="+JSONObject.quote(css)+";document.head.appendChild(s);",null);
                checkReady(0);
            }
        });
        web.loadUrl(ORIGIN+"index.html");
    }
    private void checkReady(int attempt) {
        if(disposed)return;
        web.evaluateJavascript("document.getElementById('gen').textContent", value -> {
            if(disposed)return;
            if (!value.equals("\"\"") && !value.equals("null")) {
                ready=true; status.setVisibility(GONE);
                android.util.Log.i("CanvasLife","WASM ready generation="+value);
                postDelayed(sample,250);
            } else if(attempt<20) postDelayed(() -> checkReady(attempt+1),250);
            else status.setText("Conway’s WASM did not start; check CanvasLife logs");
        });
    }
    void control(String id) {
        if (!ready) return;
        if (!id.equals("play-pause") && !id.equals("random") && !id.equals("clear")) throw new IllegalArgumentException("Unknown Life control");
        restarting=false; settlement.reset(); status.setVisibility(GONE);
        web.evaluateJavascript("document.getElementById("+JSONObject.quote(id)+").click();document.getElementById('gen').textContent",value -> android.util.Log.i("CanvasLife","control="+id+" generation="+value));
    }
    void setActive(boolean active) {
        this.active=active;
        if(active) { web.onResume(); web.resumeTimers(); }
        else { web.onPause(); web.pauseTimers(); }
    }
    private void sampleLife() {
        if(disposed)return;
        if(!active || restarting){postDelayed(sample,250);return;}
        // Downsample one center per cell; no full-resolution GPU readback.
        web.evaluateJavascript("(function(){var c=document.getElementById('game-of-life-canvas');var s=window.canvasLifeSample||(window.canvasLifeSample=document.createElement('canvas'));s.width=64;s.height=64;var x=s.getContext('2d',{willReadFrequently:true});x.imageSmoothingEnabled=false;x.drawImage(c,0,0,64,64);var p=x.getImageData(0,0,64,64).data,b='';for(var i=0;i<p.length;i+=4)b+=p[i+1]>80?'1':'0';return JSON.stringify({generation:Number(document.getElementById('gen').textContent),state:b});})()",value -> {
            if(disposed)return;
            try{
                JSONObject snapshot=new JSONObject((String)new org.json.JSONTokener(value).nextValue());
                if(settlement.observe(snapshot.getLong("generation"),snapshot.getString("state"))){
                    restarting=true;status.setText("Settled / repeating · reseeding…");status.setVisibility(VISIBLE);
                    android.util.Log.i("CanvasLife","settled generation="+snapshot.getLong("generation"));
                    postDelayed(() -> {if(!disposed && active && restarting)control("random");else restarting=false;},1500);
                }
            }catch(Exception error){status.setText("Life detector failed: "+error.getMessage());status.setVisibility(VISIBLE);android.util.Log.e("CanvasLife","Detector failed",error);}
            postDelayed(sample,250);
        });
    }
    @Override protected void onDetachedFromWindow() {
        disposed=true;removeCallbacks(sample);
        web.stopLoading(); web.loadUrl("about:blank"); web.destroy();
        super.onDetachedFromWindow();
    }
}

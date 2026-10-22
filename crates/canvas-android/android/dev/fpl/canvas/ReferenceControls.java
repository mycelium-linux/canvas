package dev.fpl.canvas;

import android.app.Activity;
import android.graphics.Color;
import android.graphics.drawable.GradientDrawable;
import android.os.Handler;
import android.view.Gravity;
import android.view.KeyEvent;
import android.view.View;
import android.widget.*;
import org.json.*;
import java.util.ArrayList;
import java.util.concurrent.*;

/** Remote surface only. Rust talks to playback/matrix owners; no shell or audio logic. */
final class ReferenceControls extends LinearLayout {
    private static native String request(String origin,String action,String id);
    private final Activity host;
    private final String playerOrigin,studioOrigin,mpdInterface;
    private final Handler ui=new Handler();
    private final ExecutorService worker=Executors.newSingleThreadExecutor();
    private final TextView info,selection;
    private final LinearLayout bank;
    private final Button play,pause,stop,previous,next,mute,solo;
    private final ArrayList<Button> transports=new ArrayList<>();
    private JSONArray inputs=new JSONArray();
    private String selected="";
    private boolean busy=false,active=false,disposed=false;
    private final int text,accent,surface;

    ReferenceControls(Activity host,String playerOrigin,String studioOrigin,String mpdInterface,JSONObject palette,Runnable close)throws Exception {
        super(host);this.host=host;this.playerOrigin=playerOrigin;this.studioOrigin=studioOrigin;this.mpdInterface=mpdInterface;
        text=Color.parseColor(palette.getString("text"));accent=Color.parseColor(palette.getString("primary"));surface=Color.parseColor(palette.getString("surface"));
        setOrientation(VERTICAL);setPadding(dp(24),dp(14),dp(24),dp(14));setBackgroundColor(surface);
        LinearLayout heading=new LinearLayout(host);heading.setGravity(Gravity.CENTER_VERTICAL);
        Button back=button("‹ Close");back.setOnClickListener(v->close.run());heading.addView(back);
        selection=label("MPD · playback",20);heading.addView(selection,new LinearLayout.LayoutParams(0,-2,1));addView(heading);
        bank=new LinearLayout(host);bank.setPadding(0,dp(10),0,dp(10));addView(bank);
        LinearLayout actions=new LinearLayout(host);actions.setGravity(Gravity.CENTER_VERTICAL);
        previous=button("|◀");play=button("▶ Play");pause=button("Ⅱ Pause");stop=button("■ Stop");next=button("▶|");
        previous.setContentDescription("Previous track");next.setContentDescription("Next track");
        previous.setOnClickListener(v->transport("previous"));play.setOnClickListener(v->transport("play"));pause.setOnClickListener(v->transport("pause"));stop.setOnClickListener(v->transport("stop"));next.setOnClickListener(v->transport("next"));
        for(Button b:new Button[]{previous,play,pause,stop,next}){transports.add(b);actions.addView(b);}
        mute=button("Mute");solo=button("Solo refs");mute.setOnClickListener(v->input("mute"));solo.setOnClickListener(v->input("solo"));actions.addView(mute);actions.addView(solo);addView(actions);
        info=label("Connecting…",16);info.setPadding(0,dp(10),0,0);addView(info);
        setButtons();
    }
    private int dp(float n){return Math.round(n*getResources().getDisplayMetrics().density);}
    private TextView label(String value,int size){TextView v=new TextView(host);v.setText(value);v.setTextSize(size);v.setTextColor(text);return v;}
    private GradientDrawable plate(boolean focused){GradientDrawable g=new GradientDrawable();g.setColor(surface);g.setCornerRadius(dp(5));g.setStroke(dp(focused?2:1),focused?accent:Color.DKGRAY);return g;}
    private Button button(String value){Button b=new Button(host);b.setId(View.generateViewId());b.setFocusableInTouchMode(true);b.setText(value);b.setAllCaps(false);b.setTextSize(17);b.setTextColor(text);b.setBackgroundTintList(null);b.setStateListAnimator(null);b.setElevation(0);b.setBackground(plate(false));b.setPadding(dp(14),dp(4),dp(14),dp(4));b.setOnFocusChangeListener((v,f)->v.setBackground(plate(f)));return b;}
    void enter(){active=true;play.requestFocus();refresh();}
    void leave(){active=false;}
    void dispose(){disposed=true;worker.shutdownNow();ui.removeCallbacksAndMessages(null);}
    private boolean ownsTransport(){return selected.isEmpty()||selected.equals(mpdInterface);}
    private void setButtons(){for(Button b:transports)b.setEnabled(!busy&&ownsTransport()&&!playerOrigin.isEmpty());mute.setEnabled(!busy&&!selected.isEmpty()&&!studioOrigin.isEmpty());solo.setEnabled(mute.isEnabled());}
    private interface Reply{void accept(JSONObject result)throws Exception;}
    private void call(String origin,String action,String id,Reply reply){
        if(busy||disposed)return;busy=true;setButtons();info.setText("Working…");
        worker.execute(()->{
            JSONObject value;try{value=new JSONObject(request(origin,action,id));}catch(Exception e){value=new JSONObject();try{value.put("error",e.getMessage());}catch(Exception ignored){android.util.Log.e("CanvasPlayback","Failed to construct error",ignored);}}
            final JSONObject result=value;
            ui.post(()->{if(disposed)return;busy=false;setButtons();try{if(result.has("error"))throw new IllegalStateException(result.getString("error"));reply.accept(result);}catch(Exception e){info.setText("Control failed: "+e.getMessage()+" · outcome may be unknown; no automatic retry");}});
        });
    }
    private void player(JSONObject reply)throws Exception {
        JSONObject p=reply.getJSONObject("player");String state=p.getString("state");
        info.setText("MPD · "+state+" · "+p.getString("artist")+" — "+p.getString("title"));
        play.setText(state.equals("play")?"▶ Playing":"▶ Play");pause.setText(state.equals("pause")?"Ⅱ Paused":"Ⅱ Pause");
    }
    private void refresh(){
        if(!studioOrigin.isEmpty())call(studioOrigin,"inputs","",result->{inputs=result.getJSONArray("inputs");renderBank();if(ownsTransport()&&!playerOrigin.isEmpty())transport("status");});
        else if(!playerOrigin.isEmpty())transport("status");
    }
    private void renderBank()throws Exception {
        bank.removeAllViews();boolean retained=false;
        for(int n=0;n<inputs.length();n++) {
            JSONObject source=inputs.getJSONObject(n);String id=source.getString("id");
            if(id.equals(selected))retained=true;
            Button b=button(id.replaceFirst("^reference-","").toUpperCase(java.util.Locale.ROOT));
            b.setOnClickListener(v->{selected=id;selection.setText(b.getText()+" · reference input");updateInputState();setButtons();if(ownsTransport())transport("status");else info.setText("Mute / reference solo available · transport controlled by source device");});bank.addView(b);
        }
        if(!retained&&!mpdInterface.isEmpty())for(int n=0;n<inputs.length();n++)if(inputs.getJSONObject(n).getString("id").equals(mpdInterface)){selected=mpdInterface;break;}
        updateInputState();setButtons();
    }
    private void updateInputState(){
        try{for(int n=0;n<inputs.length();n++){JSONObject i=inputs.getJSONObject(n);if(i.getString("id").equals(selected)){mute.setText(i.getBoolean("muted")?"Unmute":"Mute");solo.setText(i.getBoolean("soloed")?"Unsolo refs":"Solo refs");return;}}}catch(Exception e){info.setText("Input state unavailable");}
    }
    private void input(String action){if(selected.isEmpty())return;call(studioOrigin,action,selected,result->{inputs=result.getJSONArray("inputs");updateInputState();info.setText("Reference input state confirmed");});}
    void transport(String command){
        if(!ownsTransport()){info.setText("This source does not advertise remote transport");return;}
        if(playerOrigin.isEmpty()){info.setText("Playback owner is not configured");return;}
        call(playerOrigin,"player."+command,"",this::player);
    }
    boolean media(int key,KeyEvent event){
        String command;
        switch(key){
            case KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE:command="toggle";break;
            case KeyEvent.KEYCODE_MEDIA_PLAY:command="play";break;
            case KeyEvent.KEYCODE_MEDIA_PAUSE:command="pause";break;
            case KeyEvent.KEYCODE_MEDIA_STOP:command="stop";break;
            case KeyEvent.KEYCODE_MEDIA_NEXT:command="next";break;
            case KeyEvent.KEYCODE_MEDIA_PREVIOUS:command="previous";break;
            case KeyEvent.KEYCODE_VOLUME_MUTE:case KeyEvent.KEYCODE_MUTE:if(event.getRepeatCount()==0)input("mute");return true;
            default:return false;
        }
        if(event.getRepeatCount()==0)transport(command);return true;
    }
}

package dev.fpl.canvas;

import android.app.Activity;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.Color;
import android.graphics.drawable.GradientDrawable;
import android.os.Handler;
import android.view.Gravity;
import android.view.View;
import android.view.ViewParent;
import android.widget.*;
import org.json.*;
import java.util.concurrent.*;

/** Platform paint/input adapter. Rust owns catalog admission, playback and GFX layout. */
final class LibraryView extends FrameLayout {
    private static final int PAGE_SIZE=24;
    private static native String request(String origin, String action, String artist, String album, int offset);
    private static native String layout(int width, int height, int count);
    private static native String crateLayout(int width,int height,int count);
    private static native byte[] artwork(String origin, String artist, String album);
    private static native int accent(byte[] rgba);
    private final Activity host;
    private final String origin;
    private final RecordSounds sounds;
    private final Handler ui = new Handler();
    private final ThreadPoolExecutor worker = new ThreadPoolExecutor(1,1,30,TimeUnit.SECONDS,new ArrayBlockingQueue<Runnable>(12));
    private final ThreadPoolExecutor artWorker = new ThreadPoolExecutor(1,1,30,TimeUnit.SECONDS,new ArrayBlockingQueue<Runnable>(24));
    private final ThreadPoolExecutor detailWorker = new ThreadPoolExecutor(1,1,30,TimeUnit.SECONDS,new ArrayBlockingQueue<Runnable>(1));
    private final int foreground, muted, accent, surface;
    private final TextView status, title, artist, trackList;
    private final FrameLayout shelf;
    private final LinearLayout detail;
    private final Button back, play, enqueue, previous, next;
    private final java.util.ArrayList<View> covers=new java.util.ArrayList<>();
    private final java.util.ArrayList<Button> tabs=new java.util.ArrayList<>();
    private volatile int generation=0, selected=-1;
    private int offset=0;
    private volatile boolean disposed=false;
    private boolean busy=false;
    private boolean detailMode=false;
    private boolean active=false;
    private boolean compact;
    private Runnable pendingFocus;
    private View returnFocus;
    private final java.util.HashMap<String,String> trackCache=new java.util.HashMap<>();
    private static final class Art { final Bitmap image; final int color; Art(Bitmap image,int color){this.image=image;this.color=color;} }
    private final android.util.LruCache<String,Art> artCache=new android.util.LruCache<String,Art>(16*1024*1024) {
        @Override protected int sizeOf(String key,Art value){return value.image.getAllocationByteCount();}
    };
    private JSONArray albums=new JSONArray();
    private int count=0;
    private int directoryColumns=1;
    private final java.util.ArrayDeque<Integer> pageHistory=new java.util.ArrayDeque<>();
    private String section="albums", artistFilter="";
    private volatile int selectionRevision=0;

    LibraryView(Activity host, String origin, JSONObject palette, Runnable close) throws Exception {
        super(host); this.host=host; this.origin=origin;
        sounds=new RecordSounds(host,host.getPreferences(0).getBoolean("record_sfx",true));
        compact=host.getPreferences(0).getBoolean("record_compact",true);
        foreground=0xff292721; muted=0xff68645b;
        accent=Color.parseColor(palette.getString("primary")); surface=0xffe5dfd2;
        setBackgroundColor(0xfff1ecdf);
        LinearLayout bar=new LinearLayout(host); bar.setGravity(Gravity.CENTER_VERTICAL); bar.setPadding(dp(28),0,dp(28),0);
        back=button("‹ Close"); back.setOnClickListener(v -> { if(!goBack())close.run(); }); bar.addView(back);
        for(String category:new String[]{"playlists","artists","albums"}) {
            Button tab=button(category.equals("albums")?"Downloaded":category.equals("artists")?"Artists":"Playlists");
            tabs.add(tab);
            tab.setOnClickListener(v->{if(busy)return; pageHistory.clear();section=category;artistFilter="";load(0);});bar.addView(tab);
        }
        View space=new View(host);bar.addView(space,new LinearLayout.LayoutParams(0,1,1));
        Button density=button(compact?"▦":"▤");density.setContentDescription("Toggle compact record crate");
        density.setOnClickListener(v->toggleDensity(density));bar.addView(density);tabs.add(density);
        Button sound=button(host.getPreferences(0).getBoolean("record_sfx",true)?"♪":"♪ Off");sound.setContentDescription("Toggle record selection sounds");
        sound.setOnClickListener(v->{boolean enabled=!host.getPreferences(0).getBoolean("record_sfx",true);host.getPreferences(0).edit().putBoolean("record_sfx",enabled).apply();sounds.enabled(enabled);sound.setText(enabled?"♪":"♪ Off");});bar.addView(sound);tabs.add(sound);
        previous=button("‹"); next=button("›"); previous.setContentDescription("Previous albums"); next.setContentDescription("Next albums");
        previous.setOnClickListener(v -> { if(!busy&&!pageHistory.isEmpty()) load(pageHistory.pop()); });
        next.setOnClickListener(v -> { if(!busy && count>0){pageHistory.push(offset);load(offset+count);} });
        bar.addView(previous); bar.addView(next); addView(bar,new LayoutParams(-1,dp(64)));
        shelf=new FrameLayout(host); LayoutParams body=new LayoutParams(-1,-1); body.topMargin=dp(86); body.leftMargin=dp(36); body.rightMargin=dp(36); body.bottomMargin=dp(50); addView(shelf,body);
        detail=new LinearLayout(host); detail.setOrientation(LinearLayout.VERTICAL); detail.setPadding(dp(24),dp(20),dp(24),dp(20)); detail.setBackground(plate(surface,0,0));
        title=label("Select a record",30,foreground); artist=label("",18,muted); trackList=label("",16,muted);
        detail.addView(title); detail.addView(artist); LinearLayout actions=new LinearLayout(host); actions.setPadding(0,dp(20),0,dp(20));
        play=button("▶ Play"); enqueue=button("+ Queue"); play.setEnabled(false); enqueue.setEnabled(false);
        play.setOnClickListener(v -> select("play")); enqueue.setOnClickListener(v -> select("enqueue")); actions.addView(play); actions.addView(enqueue); detail.addView(actions);
        play.setOnFocusChangeListener(this::detailFocus); enqueue.setOnFocusChangeListener(this::detailFocus);
        ScrollView scroll=new ScrollView(host); scroll.addView(trackList); detail.addView(scroll,new LinearLayout.LayoutParams(-1,0,1));
        status=label("Connecting to Deadwax…",14,muted); LayoutParams footer=new LayoutParams(-1,dp(38),Gravity.BOTTOM); footer.leftMargin=dp(36); addView(status,footer);
        post(() -> load(0));
    }
    void enter() { active=true; back.setEnabled(true); post(() -> { if(!active||disposed)return; if(returnFocus!=null && returnFocus.isEnabled() && returnFocus.isAttachedToWindow())returnFocus.requestFocus(); else if(!covers.isEmpty())covers.get(0).requestFocus();else back.requestFocus(); }); }
    void leave() { active=false; returnFocus=findFocus(); if(pendingFocus!=null)ui.removeCallbacks(pendingFocus); }
    boolean goBack() {
        if(!artistFilter.isEmpty()) {pageHistory.clear();artistFilter="";section="artists";load(0);return true;}
        if(detailMode && selected>=0 && selected<covers.size()) {
            detailMode=false; back.setText("‹ Close"); covers.get(selected).requestFocus(); return true;
        }
        return false;
    }
    private void detailFocus(View view,boolean focused) {
        TvMotion.focus(view,focused);
        view.setBackground(plate(surface,focused?accent:surface,focused?3:1));
        if(focused){detailMode=true;back.setText("‹ Records");}
    }
    private void toggleDensity(Button button){if(busy)return;compact=!compact;host.getPreferences(0).edit().putBoolean("record_compact",compact).apply();button.setText(compact?"▦":"▤");try{if(section.equals("albums"))render(++generation);else renderDirectory(section,++generation);}catch(Exception e){status.setText("Layout unavailable: "+e.getMessage());}}
    @Override public View focusSearch(View focused,int direction) {
        View next=super.focusSearch(focused,direction);
        View candidate=next;
        while(candidate!=null) {
            if(candidate==this)return next;
            ViewParent parent=candidate.getParent(); candidate=parent instanceof View?(View)parent:null;
        }
        return focused;
    }
    private int dp(float value) { return Math.round(value*getResources().getDisplayMetrics().density); }
    private TextView label(String value,int size,int color) { TextView v=new TextView(host); v.setText(value); v.setTextSize(size); v.setTextColor(color); return v; }
    private GradientDrawable plate(int color,int stroke,int width) { GradientDrawable v=new GradientDrawable(); v.setColor(color); v.setCornerRadius(dp(12)); if(width>0)v.setStroke(dp(width),stroke); return v; }
    private Button button(String text) { Button v=new Button(host); v.setId(View.generateViewId()); v.setFocusableInTouchMode(true); v.setText(text); v.setTextSize(16); v.setAllCaps(false); v.setTextColor(foreground); v.setBackgroundTintList(null); v.setStateListAnimator(null); v.setElevation(0); v.setBackground(plate(surface,surface,1)); v.setPadding(dp(18),dp(8),dp(18),dp(8)); v.setOnFocusChangeListener((item,focused)-> {item.setBackground(plate(surface,focused?accent:surface,focused?3:1));TvMotion.focus(item,focused);}); return v; }
    private JSONObject reply(String raw) throws Exception { JSONObject value=new JSONObject(raw); if(value.has("error"))throw new IllegalArgumentException(value.getString("error")); return value; }
    private void job(Runnable job) { try { worker.execute(job); } catch(RejectedExecutionException e) { status.setText("Library busy — try again"); busy=false; } }
    private void load(int requested) {
        if(busy||disposed)return; busy=true; final int current=++generation; status.setText("Loading records…");
        final String category=section, filter=artistFilter;
        job(() -> { try {
            JSONArray result=reply(request(origin,category,filter,"",requested)).getJSONArray(category);
            ui.post(() -> { if(disposed||current!=generation)return; busy=false; offset=requested; albums=result; selected=-1;
                try { if(category.equals("albums"))render(current);else renderDirectory(category,current); } catch(Exception e) { status.setText("Layout unavailable: "+e.getMessage()); } });
        } catch(Exception e) { ui.post(() -> { if(!disposed&&current==generation){busy=false; status.setText("Library unavailable: "+e.getMessage());} }); } });
    }
    private void renderDirectory(String category,int current) throws Exception {
        if(pendingFocus!=null)ui.removeCallbacks(pendingFocus);
        artWorker.getQueue().clear();covers.clear();shelf.removeAllViews();detailMode=false;
        float density=getResources().getDisplayMetrics().density;
        JSONObject geometry=crateGeometry(false,Math.min(24,albums.length()));
        JSONArray cards=geometry.getJSONArray("cards");directoryColumns=geometry.getInt("columns");count=cards.length();
        previous.setEnabled(!pageHistory.isEmpty());next.setEnabled(albums.length()>count||count==24);
        for(int i=0;i<count;i++) {
            final int index=i;
            String name=category.equals("artists")?albums.getString(i):albums.getJSONObject(i).getJSONObject("playlist").getString("name");
            String subtitle=category.equals("artists")?"":albums.getJSONObject(i).getJSONObject("playlist").optInt("track_count")+" tracks";
            RecordTile row=new RecordTile(host,cards.getJSONObject(i),name,subtitle,foreground,muted,surface,accent);
            row.compact(compact);
            shelf.addView(row,rect(cards.getJSONObject(i).getJSONObject("bounds")));covers.add(row);
            row.setOnFocusChangeListener((v,focused)->{row.focused(focused);if(focused)status.setText(name+(subtitle.isEmpty()?"":" · "+subtitle));});
            loadDirectoryArt(row,category,i,current);
            row.setOnClickListener(v->{try {
                sounds.play();
                if(category.equals("artists")){pageHistory.clear();artistFilter=albums.getString(index);section="albums";load(0);}
                else {
                    JSONObject list=albums.getJSONObject(index).getJSONObject("playlist");
                    final String owner=albums.getJSONObject(index).getString("owner"),playlistName=list.getString("name");
                    final int revision=++selectionRevision;
                    status.setText("Opening "+playlistName+"…");
                    detailWorker.getQueue().clear();detailWorker.execute(()->{try {
                        JSONArray tracks=reply(request(origin,"playlist.tracks",owner,playlistName,0)).getJSONArray("tracks");StringBuilder text=new StringBuilder(playlistName).append("\n\n");
                        for(int n=0;n<tracks.length();n++){JSONObject track=tracks.getJSONObject(n);text.append(track.optString("artist")).append(" — ").append(track.optString("title")).append("\n");}
                        ui.post(()->{if(disposed||!active||current!=generation||revision!=selectionRevision)return;
                            TextView contents=label(text.toString(),18,foreground);contents.setPadding(dp(20),dp(20),dp(20),dp(20));ScrollView reader=new ScrollView(host);reader.addView(contents);
                            new android.app.AlertDialog.Builder(host).setTitle("Playlist · first 100 tracks · metadata").setView(reader).setPositiveButton("Close",null).show();status.setText("Imported playlist · local availability is separate");
                        });
                    }catch(Exception e){ui.post(()->{if(!disposed&&current==generation&&revision==selectionRevision)status.setText("Playlist unavailable: "+e.getMessage());});}});
                }
            }catch(Exception e){status.setText("Selection unavailable: "+e.getMessage());}});
        }
        status.setText(count==0?"No "+category+" in the catalog":category.equals("playlists")?"Imported playlists · availability is separate from local downloads":"Select an artist to browse downloaded albums");
        TvMotion.reveal(shelf);
        linkHeader();
        if(active){if(!covers.isEmpty())covers.get(0).requestFocus();else back.requestFocus();}
    }
    private void linkHeader() {
        java.util.ArrayList<Button> header=new java.util.ArrayList<>();header.add(back);header.addAll(tabs);
        if(previous.isEnabled())header.add(previous);if(next.isEnabled())header.add(next);
        for(int i=0;i<header.size();i++){Button item=header.get(i);item.setNextFocusLeftId(header.get(Math.max(0,i-1)).getId());item.setNextFocusRightId(header.get(Math.min(header.size()-1,i+1)).getId());item.setNextFocusUpId(item.getId());if(!covers.isEmpty())item.setNextFocusDownId(covers.get(0).getId());}
        if(!section.equals("albums"))for(int i=0;i<covers.size();i++){View row=covers.get(i);row.setNextFocusUpId(i<directoryColumns?tabs.get(0).getId():covers.get(i-directoryColumns).getId());row.setNextFocusDownId(i+directoryColumns<covers.size()?covers.get(i+directoryColumns).getId():row.getId());row.setNextFocusLeftId(i%directoryColumns==0?row.getId():covers.get(i-1).getId());row.setNextFocusRightId(i%directoryColumns==directoryColumns-1||i+1==covers.size()?row.getId():covers.get(i+1).getId());}
    }
    private void loadDirectoryArt(RecordTile tile,String category,int index,int current)throws Exception {
        final String artistName=category.equals("artists")?albums.getString(index):"";
        final JSONArray candidates=category.equals("playlists")?albums.getJSONObject(index).getJSONObject("playlist").optJSONArray("covers"):category.equals("albums")?new JSONArray().put(albums.getJSONObject(index)):null;
        artWorker.execute(()->{try {
            JSONArray choices=category.equals("artists")?reply(request(origin,"albums",artistName,"",0)).getJSONArray("albums"):candidates;
            if(choices==null)return;
            for(int i=0;i<Math.min(4,choices.length());i++) {
                if(disposed||current!=generation)return;
                JSONObject item=choices.getJSONObject(i);String a=item.getString("artist"),b=item.getString("album"),key=a+"\u0000"+b;
                Art cached=artCache.get(key);
                if(cached!=null){ui.post(()->{if(!disposed&&current==generation)tile.artwork(cached.image,cached.color);});return;}
                byte[] bytes=artwork(origin,a,b);if(bytes==null||bytes.length==0)continue;
                Art art=decodeArt(bytes);
                ui.post(()->{if(disposed||current!=generation){art.image.recycle();return;}artCache.put(key,art);tile.artwork(art.image,art.color);});return;
            }
        }catch(Exception error){ui.post(()->{if(!disposed&&current==generation)tile.setContentDescription(tile.getContentDescription()+" · artwork unavailable");});}});
    }
    private Art decodeArt(byte[] bytes)throws Exception {
        BitmapFactory.Options options=new BitmapFactory.Options();options.inJustDecodeBounds=true;BitmapFactory.decodeByteArray(bytes,0,bytes.length,options);
        if(options.outWidth<=0||options.outHeight<=0||((long)options.outWidth*options.outHeight)>32_000_000L)throw new IllegalArgumentException("Artwork dimensions invalid");
        options.inJustDecodeBounds=false;options.inSampleSize=1;while(Math.max(options.outWidth,options.outHeight)/options.inSampleSize>512)options.inSampleSize*=2;
        Bitmap bitmap=BitmapFactory.decodeByteArray(bytes,0,bytes.length,options);if(bitmap==null)throw new IllegalArgumentException("Artwork decode failed");
        Bitmap thumbnail=Bitmap.createScaledBitmap(bitmap,64,64,true);int[] pixels=new int[4096];thumbnail.getPixels(pixels,0,64,0,0,64,64);byte[] rgba=new byte[4096*4];
        for(int p=0;p<pixels.length;p++){rgba[p*4]=(byte)(pixels[p]>>16);rgba[p*4+1]=(byte)(pixels[p]>>8);rgba[p*4+2]=(byte)pixels[p];rgba[p*4+3]=(byte)(pixels[p]>>24);}if(thumbnail!=bitmap)thumbnail.recycle();
        return new Art(bitmap,accent(rgba));
    }
    private void render(int current) throws Exception {
        if(pendingFocus!=null)ui.removeCallbacks(pendingFocus);
        artWorker.getQueue().clear(); covers.clear();
        shelf.removeAllViews(); count=Math.min(PAGE_SIZE,albums.length()); float density=getResources().getDisplayMetrics().density;
        JSONObject geometry=crateGeometry(true,count);
        count=geometry.getJSONArray("cards").length();
        JSONObject panel=geometry.getJSONObject("detail"); shelf.addView(detail,rect(panel)); title.setText("Select a record"); artist.setText(""); trackList.setText(""); play.setEnabled(false); enqueue.setEnabled(false);
        previous.setEnabled(!pageHistory.isEmpty()); next.setEnabled(albums.length()>count||count==PAGE_SIZE); status.setText(count==0?"No local records in this page":"Deadwax · MPD    /    Select a cover, then Play or Queue");
        JSONArray cards=geometry.getJSONArray("cards");
        for(int i=0;i<count;i++) {
            final int index=i; JSONObject album=albums.getJSONObject(i), bounds=cards.getJSONObject(i).getJSONObject("bounds");
            RecordTile card=new RecordTile(host,cards.getJSONObject(i),album.getString("album"),album.getString("artist"),foreground,muted,surface,accent);covers.add(card);
            card.compact(compact);
            card.setOnFocusChangeListener((v,focused)-> {card.focused(focused);if(focused)focus(index,current); });
            card.setOnClickListener(v -> { sounds.play();focus(index,current); play.requestFocus(); }); shelf.addView(card,rect(bounds));
            loadDirectoryArt(card,"albums",i,current);
        }
        if(count>0) {
            int columns=1; while(columns<count && cards.getJSONObject(columns).getJSONObject("bounds").getDouble("y")==cards.getJSONObject(0).getJSONObject("bounds").getDouble("y"))columns++;
            for(int i=0;i<count;i++) {
                View card=covers.get(i);
                card.setNextFocusLeftId(i%columns==0?card.getId():covers.get(i-1).getId());
                card.setNextFocusRightId(i%columns==columns-1||i+1==count?play.getId():covers.get(i+1).getId());
                card.setNextFocusUpId(i<columns?back.getId():covers.get(i-columns).getId());
                card.setNextFocusDownId(i+columns<count?covers.get(i+columns).getId():card.getId());
            }
            back.setNextFocusDownId(covers.get(0).getId()); previous.setNextFocusDownId(covers.get(0).getId()); next.setNextFocusDownId(covers.get(Math.min(columns-1,count-1)).getId());
            play.setNextFocusRightId(enqueue.getId()); enqueue.setNextFocusLeftId(play.getId());
            play.setNextFocusUpId(next.getId()); enqueue.setNextFocusUpId(next.getId());
        }
        back.setNextFocusLeftId(back.getId()); back.setNextFocusUpId(back.getId());
        back.setNextFocusRightId(previous.isEnabled()?previous.getId():next.getId());
        previous.setNextFocusLeftId(back.getId()); previous.setNextFocusRightId(next.getId()); next.setNextFocusLeftId(previous.isEnabled()?previous.getId():back.getId()); next.setNextFocusRightId(next.getId());
        TvMotion.reveal(shelf);
        linkHeader();
        if(active){if(count>0)covers.get(0).requestFocus();else back.requestFocus();}
    }
    /** Density is a host logical-coordinate transform; GFX remains the sole layout owner. */
    private JSONObject crateGeometry(boolean withDetail,int requested)throws Exception {
        float scale=compact?.76f:1f,density=getResources().getDisplayMetrics().density;
        int width=(int)Math.floor(shelf.getWidth()/density/scale),height=(int)Math.floor(shelf.getHeight()/density/scale);
        JSONObject geometry=reply(withDetail?layout(width,height,requested):crateLayout(width,height,requested));
        scaleGeometry(geometry,scale);return geometry;
    }
    private void scaleGeometry(JSONObject object,float scale)throws Exception {
        java.util.Iterator<String> keys=object.keys();while(keys.hasNext()){String key=keys.next();Object value=object.get(key);
            if(value instanceof JSONObject)scaleGeometry((JSONObject)value,scale);
            else if(value instanceof JSONArray){JSONArray array=(JSONArray)value;for(int i=0;i<array.length();i++)if(array.get(i) instanceof JSONObject)scaleGeometry(array.getJSONObject(i),scale);}
            else if(key.equals("x")||key.equals("y")||key.equals("width")||key.equals("height"))object.put(key,object.getDouble(key)*scale);
        }
    }
    private LayoutParams rect(JSONObject value) throws Exception { LayoutParams p=new LayoutParams(dp((float)value.getDouble("width")),dp((float)value.getDouble("height"))); p.leftMargin=dp((float)value.getDouble("x")); p.topMargin=dp((float)value.getDouble("y")); return p; }
    private void focus(int index,int current) {
        detailMode=false; back.setText("‹ Close");
        if(index==selected||busy||disposed)return; selected=index;final int revision=++selectionRevision;
        try { JSONObject record=albums.getJSONObject(index); title.setText(record.getString("album")); artist.setText(record.getString("artist")+" · "+record.getInt("tracks")+" tracks"); trackList.setText("Loading track list…"); play.setEnabled(true); enqueue.setEnabled(true);
            String a=record.getString("artist"),b=record.getString("album");
            play.setNextFocusLeftId(covers.get(index).getId());
            String key=record.toString();
            if(pendingFocus!=null)ui.removeCallbacks(pendingFocus);
            if(trackCache.containsKey(key)) {trackList.setText(trackCache.get(key));return;}
            pendingFocus=()->{detailWorker.getQueue().clear();try {detailWorker.execute(()->{ if(disposed||current!=generation||selected!=index||revision!=selectionRevision)return;
                try { JSONArray tracks=reply(request(origin,"tracks",a,b,0)).getJSONArray("tracks"); StringBuilder text=new StringBuilder(); for(int n=0;n<tracks.length();n++){JSONObject t=tracks.getJSONObject(n);text.append(t.getString("position")).append("   ").append(t.getString("title")).append("\n\n");} final String value=text.toString(); ui.post(()->{if(!disposed&&current==generation&&selected==index&&revision==selectionRevision){if(trackCache.size()>=24)trackCache.clear();trackCache.put(key,value);trackList.setText(value);}}); }
                catch(Exception e){ui.post(()->{if(!disposed&&current==generation&&selected==index&&revision==selectionRevision)trackList.setText("Tracks unavailable: "+e.getMessage());});} });}catch(RejectedExecutionException e){trackList.setText("Track list busy");}};
            ui.postDelayed(pendingFocus,120);
        }catch(Exception e){status.setText("Selection unavailable");}
    }
    private void select(String action) {
        if(busy||disposed||selected<0)return; busy=true; play.setEnabled(false); enqueue.setEnabled(false); status.setText(action.equals("play")?"Starting on MPD…":"Adding to queue…");
        try { JSONObject record=albums.getJSONObject(selected); String a=record.getString("artist"),b=record.getString("album");
            job(()->{ try { reply(request(origin,action,a,b,0)); ui.post(()->{if(!disposed){busy=false;play.setEnabled(true);enqueue.setEnabled(true);status.setText(action.equals("play")?"▶ Started MPD · "+b:"Added to MPD queue · "+b);}}); }
            catch(Exception e){ui.post(()->{if(!disposed){busy=false;play.setEnabled(true);enqueue.setEnabled(true);status.setText("Playback failed: "+e.getMessage()+" — queue may have changed; do not retry blindly");}});} });
        }catch(Exception e){busy=false;status.setText("Selection unavailable");}
    }
    void dispose() { disposed=true; generation++; worker.shutdownNow(); artWorker.shutdownNow(); detailWorker.shutdownNow();sounds.dispose();ui.removeCallbacksAndMessages(null); }
}

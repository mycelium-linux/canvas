local presenter = assert(loadfile(arg[1]))()
local loading = presenter.render(1000)
assert(loading.data.state == "loading" and loading.widget.nodes[1].detail == "—")
local snapshot = {schema_version=1,mode="snapshot",host_id="neo",provider="codex",
    instance_id="owner-1",sequence=1,observed_at_unix_ms=1000,sessions={
        {id="a",state="active"},{id="a",state="active"},{id="b",state="idle"},
        {id="c",state="unknown"},{id="d",state="ended"}}}
presenter.receive(snapshot,1000)
local live=presenter.render(2000)
assert(live.widget.nodes[1].detail=="1" and live.widget.nodes[2].detail=="1"
    and live.widget.nodes[3].detail=="1" and live.data.state=="partial")
local stale=presenter.render(32000)
assert(stale.data.state=="stale" and stale.widget.nodes[1].detail=="0"
    and stale.widget.nodes[2].detail=="0" and stale.widget.nodes[3].detail=="3")
local replay={schema_version=1,mode="snapshot",host_id="neo",provider="codex",
    instance_id="owner-1",sequence=0,observed_at_unix_ms=2000,sessions={}}
presenter.receive(replay,2000)
assert(presenter.render(2000).widget.nodes[1].detail=="1")
assert(not pcall(presenter.receive,snapshot,0-10000))
print("agent dashboard: loading/live/stale/replay/dedup tests passed")

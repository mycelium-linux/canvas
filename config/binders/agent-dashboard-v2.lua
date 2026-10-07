-- Canvas-only presentation of agent.sessions.v2. No IO/provider parsing.
local M, owners = {}, {}
local function text(value, limit)
    return type(value) == "string" and #value <= limit and not value:find("[%c]")
end
local function key(value) return value.host_id .. "\0" .. value.provider end
function M.receive(value, now_ms)
    assert(type(value) == "table" and value.schema_version == 2, "invalid attention contract")
    assert(value.mode == "snapshot" or value.mode == "event", "invalid update mode")
    assert(text(value.host_id,256) and text(value.provider,256) and text(value.instance_id,256), "invalid owner scope")
    assert(type(value.sequence) == "number" and value.sequence >= 0 and value.sequence % 1 == 0, "invalid sequence")
    assert(type(value.observed_at_unix_ms) == "number" and value.observed_at_unix_ms >= 0
        and value.observed_at_unix_ms % 1 == 0 and value.observed_at_unix_ms <= now_ms+5000, "invalid timestamp")
    assert(type(value.sessions) == "table" and #value.sessions <= 512, "invalid roster")
    local id = key(value)
    local old = owners[id]
    if old and old.instance == value.instance_id and value.sequence <= old.sequence then return end
    if old and value.observed_at_unix_ms < old.observed then return end
    if not old then local count=0;for _ in pairs(owners) do count=count+1 end
        assert(count<64,"owner budget exceeded") end
    local rows, seen = {}, {}
    if value.mode == "event" and old and old.instance == value.instance_id then
        for session_id, row in pairs(old.rows) do rows[session_id] = row end
    end
    for _, session in ipairs(value.sessions) do
        assert(text(session.id,256) and #session.id>0 and not seen[session.id], "invalid/duplicate session")
        seen[session.id]=true
        assert(session.state == "active" or session.state == "idle" or session.state == "unknown" or session.state == "ended", "invalid state")
        assert(session.project == nil or text(session.project,160), "invalid project label")
        assert(session.task == nil or text(session.task,160), "invalid task label")
        if session.attention then local a=session.attention
            assert(a.reason == "input" or a.reason == "approval", "invalid attention reason")
            assert(a.timing == "first_observed" and type(a.since_unix_ms)=="number"
                and a.since_unix_ms >= 0 and a.since_unix_ms % 1 == 0
                and a.since_unix_ms <= value.observed_at_unix_ms, "invalid wait timestamp") end
        if session.state == "ended" then rows[session.id]=nil
        else rows[session.id]={session=session,observed=value.observed_at_unix_ms} end
    end
    local count=0;for _ in pairs(rows) do count=count+1 end
    assert(count<=512,"retained roster budget exceeded")
    owners[id]={instance=value.instance_id,sequence=value.sequence,observed=value.observed_at_unix_ms,
        rows=rows,host=value.host_id,provider=value.provider}
end
local function age(ms)
    local seconds=math.floor(math.max(0,ms)/1000)
    if seconds<60 then return tostring(seconds).."s" end
    if seconds<3600 then return tostring(math.floor(seconds/60)).."m" end
    return tostring(math.floor(seconds/3600)).."h "..tostring(math.floor(seconds/60)%60).."m"
end
function M.render(now_ms)
    local counts={active=0,idle=0,unknown=0,input=0,approval=0}
    local roster, newest, stale, owner_count = {},0,false,0
    for _,owner in pairs(owners) do
        owner_count=owner_count+1;newest=math.max(newest,owner.observed)
        for _,entry in pairs(owner.rows) do
            local session=entry.session
            local fresh=now_ms-entry.observed<=30000 and now_ms-entry.observed>=-5000
            local status=not fresh and "unknown" or session.attention and session.attention.reason or session.state
            counts[status]=counts[status]+1
            if not fresh then stale=true end
            roster[#roster+1]={session=session,status=status,owner=owner,fresh=fresh}
        end
        if now_ms-owner.observed>30000 then stale=true end
    end
    local priority={input=0,approval=0,active=1,unknown=2,idle=3}
    table.sort(roster,function(a,b)
        if priority[a.status]~=priority[b.status] then return priority[a.status]<priority[b.status] end
        local aa=a.fresh and a.session.attention and a.session.attention.since_unix_ms or now_ms
        local bb=b.fresh and b.session.attention and b.session.attention.since_unix_ms or now_ms
        if aa~=bb then return aa<bb end
        return a.owner.host .. a.owner.provider .. a.session.id < b.owner.host .. b.owner.provider .. b.session.id
    end)
    local nodes={}
    for i=1,math.min(#roster,4) do
        local row=roster[i];local s=row.session
        local state=(row.status=="input" or row.status=="approval") and "warning" or row.status=="active" and "good" or "neutral"
        local status=row.status=="input" and "Needs response" or row.status=="approval" and "Needs approval" or row.status
        local timing=row.fresh and s.attention and (" · seen waiting "..age(now_ms-s.attention.since_unix_ms)) or ""
        nodes[#nodes+1]={label=s.project or "Unassigned",state=state,
            detail=status..timing.."\n"..(s.task or ("Unnamed session "..s.id:sub(1,8))).."\n"..row.owner.provider.." @ "..row.owner.host}
    end
    if #nodes==0 then nodes={{label="Sessions",detail=owner_count==0 and "Waiting for owner telemetry" or "No loaded sessions",state="neutral"}} end
    local function count(label,n,state) return {label=label,detail=owner_count>0 and tostring(n) or "—",state=state} end
    local data={state=owner_count==0 and "loading" or stale and "stale" or counts.unknown>0 and "partial" or "ready",
        source="unibus:agent.sessions.v2",provenance="live",detail=owner_count.." owner(s) · explicit status"}
    return {widget={kind="status_list",title="Agents",nodes={
        count("Needs input",counts.input,counts.input>0 and "warning" or "neutral"),
        count("Needs approval",counts.approval,counts.approval>0 and "warning" or "neutral"),
        count("Active",counts.active,"good"),count("Idle",counts.idle,"neutral"),
        count("Unknown",counts.unknown,counts.unknown>0 and "warning" or "neutral")}},
        visual={view="attention",panel={input=counts.input,approval=counts.approval,active=counts.active,
            idle=counts.idle,unknown=counts.unknown,fresh=owner_count>0 and not stale}},
        details={widget={kind="status_list",title="Agent attention · "..#roster.." sessions",nodes=nodes}},
        data=data,observed_at_unix_ms=newest}
end
return M

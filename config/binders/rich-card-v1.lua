local p=event.payload
if type(p)~="table" then error("rich-card-v1 requires payload") end
local format=tostring(p.format or "markdown")
if format~="markdown" and format~="code" and format~="diff" then error("unsupported rich card format") end
local citations=canvas.array()
for _,citation in ipairs(p.citations or {}) do citations[#citations+1]={label=tostring(citation.label or "source"),url=tostring(citation.url or "")} end
return {kind="snapshot",widget={kind="rich_card",title=tostring(p.title or "Briefing"),format=format,body=tostring(p.body or ""),language=p.language and tostring(p.language) or nil,citations=citations},data={state=p.state or "ready",source=tostring(p.source or event.source or "briefing"),provenance="live"}}

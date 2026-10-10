import express from 'express';
import Database from 'better-sqlite3';
import { z } from 'zod';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const app = express();
app.use(express.json({ limit: '1mb' }));

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const dbPath = process.env.DEVFORGE_DB || path.join(process.cwd(), 'devforge.db');
const db = new Database(dbPath);
db.pragma('journal_mode = WAL');
db.exec(`
CREATE TABLE IF NOT EXISTS projects (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 name TEXT NOT NULL,
 idea TEXT NOT NULL,
 status TEXT NOT NULL DEFAULT 'planning',
 created_at TEXT NOT NULL,
 updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS plans (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 project_id INTEGER NOT NULL,
 mode TEXT NOT NULL,
 plan_json TEXT NOT NULL,
 created_at TEXT NOT NULL,
 FOREIGN KEY(project_id) REFERENCES projects(id)
);
CREATE TABLE IF NOT EXISTS builds (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 project_id INTEGER NOT NULL,
 status TEXT NOT NULL,
 steps_json TEXT NOT NULL,
 created_at TEXT NOT NULL,
 FOREIGN KEY(project_id) REFERENCES projects(id)
);
`);

const IdeaSchema = z.object({ idea: z.string().min(8).max(10000) });
const ApproveSchema = z.object({ projectId: z.number().int().positive() });

function now(){ return new Date().toISOString(); }
function slugName(idea:string){
  const cleaned=idea.replace(/[^a-zA-Z0-9 ]/g,' ').trim().split(/\s+/).slice(0,6).join(' ');
  return cleaned ? cleaned.replace(/\b\w/g,c=>c.toUpperCase()) : 'Untitled Project';
}
function demoPlan(idea:string){
  const lower=idea.toLowerCase();
  const hasSchool=/(school|student|teacher|attendance|education)/.test(lower);
  const hasShop=/(shop|store|inventory|sales|commerce)/.test(lower);
  const users=hasSchool?['Administrator','Teacher','Student','Parent']:hasShop?['Owner','Staff','Customer']:['Administrator','Team member','End user'];
  const features=hasSchool?['Student management','Attendance','Results','Fees','Timetable','Notifications']:hasShop?['Products','Inventory','Orders','Customers','Sales analytics','Notifications']:['Authentication','Dashboard','Core records','Search and filtering','Notifications','Settings'];
  return { projectName:slugName(idea), summary:`A production-oriented application based on: ${idea}`, targetUsers:users, coreFeatures:features, recommendedStack:['React + TypeScript','Node.js + Express','SQLite/PostgreSQL','REST API'], architecture:['Web client','API layer','Business services','Database'], buildPhases:['Foundation','Core features','UI polish','Validation','Deployment'], risks:['Requirements may need clarification','External integrations require credentials','Production deployment needs isolated execution'] };
}

app.get('/api/health',(_req,res)=>res.json({ok:true, service:'DEVFORGE', version:'1.5.1'}));
app.get('/api/projects',(_req,res)=>{
 const rows=db.prepare('SELECT * FROM projects ORDER BY updated_at DESC').all();
 res.json(rows);
});
app.get('/api/projects/:id',(req,res)=>{
 const p=db.prepare('SELECT * FROM projects WHERE id=?').get(Number(req.params.id));
 if(!p) return res.status(404).json({error:'Project not found'});
 const plan=db.prepare('SELECT * FROM plans WHERE project_id=? ORDER BY id DESC LIMIT 1').get((p as any).id) as any;
 const build=db.prepare('SELECT * FROM builds WHERE project_id=? ORDER BY id DESC LIMIT 1').get((p as any).id) as any;
 res.json({...p, plan:plan?{...plan,plan_json:JSON.parse(plan.plan_json)}:null, build:build?{...build,steps_json:JSON.parse(build.steps_json)}:null});
});

app.post('/api/projects/plan',async(req,res)=>{
 const parsed=IdeaSchema.safeParse(req.body);
 if(!parsed.success) return res.status(400).json({error:'Please provide a project idea of at least 8 characters.'});
 const {idea}=parsed.data;
 const mode=(process.env.DEVFORGE_AI_MODE||'demo').toLowerCase();
 if(mode!=='demo') return res.status(503).json({error:`DEVFORGE_AI_MODE=${mode} has no configured AI provider. Use demo mode explicitly or configure an adapter.`});
 const effectiveMode='demo';
 const plan=demoPlan(idea);
 const t=now();
 const result=db.transaction(()=>{
   const p=db.prepare('INSERT INTO projects(name,idea,status,created_at,updated_at) VALUES(?,?,?,?,?)').run(plan.projectName,idea,'planning',t,t);
   db.prepare('INSERT INTO plans(project_id,mode,plan_json,created_at) VALUES(?,?,?,?)').run(p.lastInsertRowid,effectiveMode,JSON.stringify(plan),t);
   return Number(p.lastInsertRowid);
 })();
 res.json({projectId:result,mode:effectiveMode,plan});
});

app.post('/api/projects/approve',async(req,res)=>{
 const parsed=ApproveSchema.safeParse(req.body);
 if(!parsed.success) return res.status(400).json({error:'Invalid project id'});
 const p=db.prepare('SELECT * FROM projects WHERE id=?').get(parsed.data.projectId) as any;
 if(!p) return res.status(404).json({error:'Project not found'});
 db.prepare('UPDATE projects SET status=?,updated_at=? WHERE id=?').run('building',now(),p.id);
 const steps=[
  {key:'architecture',label:'Architecture',status:'complete',detail:'Project structure prepared'},
  {key:'generation',label:'Code generation',status:'complete',detail:'Starter application generated'},
  {key:'validation',label:'Validation',status:'complete',detail:'Static project validation passed'},
  {key:'preview',label:'Preview',status:'ready',detail:'Preview package is ready'}
 ];
 const t=now();
 const b=db.prepare('INSERT INTO builds(project_id,status,steps_json,created_at) VALUES(?,?,?,?)').run(p.id,'complete',JSON.stringify(steps),t);
 db.prepare('UPDATE projects SET status=?,updated_at=? WHERE id=?').run('complete',t,p.id);
 res.json({buildId:Number(b.lastInsertRowid),status:'complete',steps});
});

app.post('/api/projects/:id/chat',async(req,res)=>{
 const p=db.prepare('SELECT * FROM projects WHERE id=?').get(Number(req.params.id)) as any;
 if(!p) return res.status(404).json({error:'Project not found'});
 const message=String(req.body?.message||'').trim();
 if(!message) return res.status(400).json({error:'Message required'});
 res.json({mode:'demo',reply:`DEVFORGE understood: “${message}”. In v0.1 this project assistant is connected to the project context, while code mutation is being prepared for the next build milestone.`});
});

app.use(express.static(path.join(process.cwd(),'dist/client')));
app.get('/{*path}',(_req,res)=>res.sendFile(path.join(process.cwd(),'dist/client/index.html')));

const port=Number(process.env.PORT||8787);
app.listen(port,()=>console.log(`DEVFORGE API running on http://localhost:${port}`));

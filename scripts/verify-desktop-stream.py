import socket,ssl,struct,subprocess,pathlib,sys,time
from PIL import Image
base=pathlib.Path.home()/'Library/Application Support/Dock'
s=socket.create_connection(('127.0.0.1',15901),timeout=15)
def read(n):
 b=b''
 while len(b)<n:
  p=s.recv(n-len(b))
  if not p:raise RuntimeError('stream closed')
  b+=p
 return b
version=read(12);s.sendall(b'RFB 003.008\n')
types=read(read(1)[0]);assert 19 in types
s.sendall(bytes([19]));v=read(2);s.sendall(v);assert read(1)==b'\0'
types=[struct.unpack('!I',read(4))[0] for _ in range(read(1)[0])];assert 262 in types
s.sendall(struct.pack('!I',262));assert read(1)==b'\1'
ctx=ssl.create_default_context(cafile=str(base/'desktop-vnc-cert.pem'))
s=ctx.wrap_socket(s,server_hostname='localhost')
password=subprocess.check_output(['ssh','-o','BatchMode=yes','-o','ConnectTimeout=8','agora-one','cat .config/dock/wayvnc/password']).strip()
user=b'dock';s.sendall(struct.pack('!II',len(user),len(password))+user+password)
assert read(4)==b'\0\0\0\0','authentication rejected'
s.sendall(b'\1');header=read(24);width,height=struct.unpack('!HH',header[:4]);name=read(struct.unpack('!I',header[20:])[0])
s.sendall(b'\0\0\0\0'+struct.pack('!BBBBHHHBBBxxx',32,24,0,1,255,255,255,16,8,0))
s.sendall(struct.pack('!BBHi',2,0,1,0))
def frame(path):
 s.sendall(struct.pack('!BBHHHH',3,0,0,0,width,height))
 canvas=Image.new('RGB',(width,height));pixels=0
 while True:
  kind=read(1)[0]
  if kind==0:
   count=struct.unpack('!xH',read(3))[0]
   for _ in range(count):
    x,y,w,h,enc=struct.unpack('!HHHHi',read(12))
    if enc!=0:raise RuntimeError('unexpected encoding '+str(enc))
    data=read(w*h*4);canvas.paste(Image.frombytes('RGB',(w,h),data,'raw','BGRX'),(x,y));pixels+=w*h
   break
  elif kind==2:continue
  elif kind==3:read(struct.unpack('!xxxI',read(7))[0])
  else:raise RuntimeError('unexpected server message '+str(kind))
 canvas.save(path);print('Received',pixels,'pixels',str(path))
frame('/private/tmp/dock-vnc-before.png')
if sys.argv[1:]==['--keyboard']:
 for key in (0xff09,0xff0d):
  s.sendall(struct.pack('!BBHI',4,1,0,key));s.sendall(struct.pack('!BBHI',4,0,0,key))
 time.sleep(.5);frame('/private/tmp/dock-vnc-keyboard.png')
if len(sys.argv)==3:
 x,y=map(int,sys.argv[1:]);s.sendall(struct.pack('!BBHH',5,1,x,y));s.sendall(struct.pack('!BBHH',5,0,x,y));time.sleep(.5)
 frame('/private/tmp/dock-vnc-after.png')
s.close()

"""Schema 2 product manifest: lstat-validated inputs and directory modes."""
import hashlib,json,stat
from pathlib import Path
FILES=['.gitignore','Cargo.toml','Cargo.lock','README.md','THIRD_PARTY_NOTICES.md']
FOLDERS=['Tests','crates','scripts','resources','automation','docs']
def inventory(root):
 root=Path(root);paths=[];directories={}
 def visit(p,directory=False):
  try:s=p.lstat()
  except FileNotFoundError:raise ValueError('missing required product input: '+str(p.relative_to(root)))
  name=str(p.relative_to(root))
  if stat.S_ISLNK(s.st_mode):raise ValueError('product symlink: '+name)
  if directory:
   if not stat.S_ISDIR(s.st_mode):raise ValueError('expected real directory: '+name)
   directories[name]=stat.S_IMODE(s.st_mode)
   for child in p.iterdir():
    if child.name=='__pycache__' or child.suffix=='.pyc':continue
    visit(child,stat.S_ISDIR(child.lstat().st_mode))
  else:
   if not stat.S_ISREG(s.st_mode):raise ValueError('expected regular file: '+name)
   paths.append(p)
 for f in FILES:visit(root/f)
 for f in FOLDERS:visit(root/f,True)
 rows=[{'path':str(p.relative_to(root)),'bytes':p.stat().st_size,'mode':stat.S_IMODE(p.stat().st_mode),'sha256':hashlib.sha256(p.read_bytes()).hexdigest()} for p in sorted(paths)]
 result={'schema':2,'missing':[],'files':rows,'directories':sorted(directories),'directory_modes':directories}
 result['sha256']=hashlib.sha256(json.dumps(result,sort_keys=True,separators=(',',':')).encode()).hexdigest()
 return result

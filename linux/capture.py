"""Acquire only the visible game window at the common observer's room marker."""
import subprocess
import re
import time
from pathlib import Path
def save(directory):
    clients=subprocess.run(['xprop','-root','_NET_CLIENT_LIST'],capture_output=True,text=True).stdout
    targets=[]
    for handle in re.findall('0x[0-9a-fA-F]+',clients):
        v=subprocess.run(['xprop','-id',handle,'_NET_WM_NAME','WM_NAME','WM_CLASS'],capture_output=True,text=True).stdout
        if re.search(r'VIctory [0-9]+ fps [0-9]+(?:\.[0-9]+)? ms/frame',v,re.I):
            targets.append((handle,v))
    if len(targets)!=1: raise RuntimeError(f'visible game-window matches: {targets}')
    (directory/'window.txt').write_text(targets[0][1])
    subprocess.run(['import','-silent','-window',targets[0][0],str(directory/'room-view.png')],check=True,timeout=10)

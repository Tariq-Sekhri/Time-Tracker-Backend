much later dashboard to accept new deivce to server

delete device cascade
logs are encpipted

devices can only upload there own device stuff

we hide uuid from other devices
auth token 
server will create uuid for cliend 
all intercations will use auth token  encpyted otken + salt 



new device flow 
client checks if server is valid 
server responds yes
client locks the server 
client request uuid and token from the server 
server give back them 

user then has options 
upload and sync there logs 
download other logs 


user wants to upload
they will push all their logs -1 (not the one currently happening) with device info 
{device{auth, uuid}, logs[]}
/upload_all_logs this will reset and delte the logs there it will be seen as  a reset??/
server will store log {device uuid, logs info}


/sync every 5 min 
{device, logs[], deleted_ids[]} 
last uploaded log id -> newest log -1 
if secesed last uploaded id = newest log id -1 other wise dont 

when client deltes a log it gets added to a delte list clear delete list 


//------------------------------ sync finished---------
/get devices (gets called everytime we go to sync or a refresh is called)
this lists the devices (not giving token)

// get logs/uuid 
clent will do promise.all() to get all the end points, 
skip logic will be applied not

// 


struct and tables 
```Rust
struct Devices {
    uuid:String,
    name:String,
    is_tracking:bool,
    is_us:bool, 
    last_sync_id:i64,
}

struct Log{
    id:i64,
    //default none
    device_uuid:Option<String>,
    app:String,
    timestamp:i64,
    duration:i64
}

```
appmetadata key = deleted_log_ids = vec<i64>
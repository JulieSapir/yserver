// Pointer motion at a fixed rate (XTest, own connection) while a second
// connection measures GetInputFocus round trips. Usage: xlat <seconds> <hz>
#include <xcb/xcb.h>
#include <xcb/xtest.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#include <unistd.h>
#include <sys/wait.h>
static double now(void){struct timespec t;clock_gettime(CLOCK_MONOTONIC,&t);return t.tv_sec+t.tv_nsec/1e9;}
static int cmp(const void*a,const void*b){double x=*(double*)a,y=*(double*)b;return x<y?-1:x>y;}
int main(int argc,char**argv){
  double secs=atof(argv[1]); int hz=atoi(argv[2]);
  pid_t pid=fork();
  if(pid==0){
    xcb_connection_t*c=xcb_connect(NULL,NULL); if(xcb_connection_has_error(c)) return 2;
    xcb_window_t root=xcb_setup_roots_iterator(xcb_get_setup(c)).data->root;
    double end=now()+secs, period=1.0/hz; long n=0;
    for(double t=now(); t<end; t+=period){
      int x=100+(int)(n*7%1700), y=100+(int)(n*3%850);
      xcb_test_fake_input(c,XCB_MOTION_NOTIFY,0,XCB_CURRENT_TIME,root,x,y,0);
      if(++n%32==0) free(xcb_get_input_focus_reply(c,xcb_get_input_focus(c),NULL)); else xcb_flush(c);
      double d=t+period-now(); if(d>0){struct timespec s={0,(long)(d*1e9)};nanosleep(&s,NULL);}
    }
    printf("motions %ld\n",n); xcb_disconnect(c); return 0;
  }
  xcb_connection_t*c=xcb_connect(NULL,NULL); if(xcb_connection_has_error(c)) return 2;
  size_t cap=1<<25,n=0; double*v=malloc(cap*sizeof(double)); double end=now()+secs;
  while(now()<end && n<cap){ double t0=now(); free(xcb_get_input_focus_reply(c,xcb_get_input_focus(c),NULL)); v[n++]=(now()-t0)*1e3; }
  waitpid(pid,NULL,0);
  qsort(v,n,sizeof(double),cmp); size_t over4=0,over8=0,over16=0; for(size_t i=0;i<n;i++){over4+=v[i]>4;over8+=v[i]>8;over16+=v[i]>16;}
  printf("rtt n=%zu p50=%.3f p90=%.3f p99=%.3f p999=%.3f max=%.3f ms  >4ms=%zu >8ms=%zu >16ms=%zu\n",n,v[n/2],v[n*9/10],v[n*99/100],v[n*999/1000],v[n-1],over4,over8,over16);
  return 0;
}

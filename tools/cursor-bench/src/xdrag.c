// Drag a top-level window by its title bar with XTest at <hz> for <seconds>,
// while a second connection samples every 5 ms how far the window trails the
// pointer. Usage: xdrag <client-window-id> <seconds> <hz>
#include <xcb/xcb.h>
#include <xcb/xtest.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <time.h>
#include <unistd.h>
#include <sys/wait.h>
static double now(void){struct timespec t;clock_gettime(CLOCK_MONOTONIC,&t);return t.tv_sec+t.tv_nsec/1e9;}
static void sleep_until(double t){double d=t-now(); if(d>0){struct timespec s={(time_t)d,(long)((d-(time_t)d)*1e9)};nanosleep(&s,NULL);}}
static int cmp(const void*a,const void*b){double x=*(double*)a,y=*(double*)b;return x<y?-1:x>y;}
static void winpos(xcb_connection_t*c,xcb_window_t root,xcb_window_t w,int*x,int*y){
  xcb_translate_coordinates_reply_t*r=xcb_translate_coordinates_reply(c,xcb_translate_coordinates(c,w,root,0,0),NULL);
  *x=r?r->dst_x:0;*y=r?r->dst_y:0;free(r);}
static void ptr(xcb_connection_t*c,xcb_window_t root,int*x,int*y){
  xcb_query_pointer_reply_t*r=xcb_query_pointer_reply(c,xcb_query_pointer(c,root),NULL);*x=r?r->root_x:0;*y=r?r->root_y:0;free(r);}
int main(int argc,char**argv){
  xcb_window_t w=strtoul(argv[1],NULL,0); double secs=atof(argv[2]); int hz=atoi(argv[3]);
  xcb_connection_t*c=xcb_connect(NULL,NULL); if(xcb_connection_has_error(c)) return 2;
  xcb_window_t root=xcb_setup_roots_iterator(xcb_get_setup(c)).data->root;
  int wx,wy; winpos(c,root,w,&wx,&wy);
  int gx=wx+60, gy=wy-12;               // a point on the title bar
  int cx=960, cy=540; double R=250;     // circle the drag follows
  xcb_test_fake_input(c,XCB_MOTION_NOTIFY,0,XCB_CURRENT_TIME,root,gx,gy,0); xcb_flush(c); usleep(300000);
  xcb_test_fake_input(c,XCB_BUTTON_PRESS,1,XCB_CURRENT_TIME,root,0,0,0); xcb_flush(c); usleep(300000);
  winpos(c,root,w,&wx,&wy); int px,py; ptr(c,root,&px,&py); int ox=px-wx, oy=py-wy; // grab offset
  pid_t pid=fork();
  if(pid==0){
    xcb_connection_t*m=xcb_connect(NULL,NULL);
    double t0=now(),period=1.0/hz; long n=0;
    // glide from the grab point to the circle, then circle
    for(double t=t0;t<t0+secs;t+=period){
      double a=2*M_PI*0.5*(t-t0); int x=cx+(int)(R*cos(a)), y=cy+(int)(R*sin(a));
      if(t-t0<0.5){double f=(t-t0)/0.5; x=(int)(gx+(x-gx)*f); y=(int)(gy+(y-gy)*f);}
      xcb_test_fake_input(m,XCB_MOTION_NOTIFY,0,XCB_CURRENT_TIME,root,x,y,0); xcb_flush(m); n++;
      sleep_until(t+period);
    }
    xcb_test_fake_input(m,XCB_BUTTON_RELEASE,1,XCB_CURRENT_TIME,root,0,0,0); xcb_flush(m);
    free(xcb_get_input_focus_reply(m,xcb_get_input_focus(m),NULL));
    printf("drag motions %ld\n",n); return 0;
  }
  size_t cap=100000,n=0; double*lag=malloc(cap*sizeof(double)),*rt=malloc(cap*sizeof(double));
  double t0=now()+0.7;                      // skip the glide
  sleep_until(t0);
  for(double t=t0;t<t0+secs-1.0 && n<cap;t+=0.005){
    double q=now(); int x,y,qx,qy; ptr(c,root,&qx,&qy); winpos(c,root,w,&x,&y); rt[n]=(now()-q)*1e3;
    lag[n++]=hypot((qx-ox)-x,(qy-oy)-y); sleep_until(t+0.005);
  }
  waitpid(pid,NULL,0);
  qsort(lag,n,sizeof(double),cmp); qsort(rt,n,sizeof(double),cmp);
  size_t over20=0,over50=0; for(size_t i=0;i<n;i++){over20+=lag[i]>20;over50+=lag[i]>50;}
  printf("samples %zu  window lag px p50=%.0f p90=%.0f p99=%.0f max=%.0f  >20px=%zu >50px=%zu  query ms p50=%.2f p99=%.2f max=%.2f\n",
    n,lag[n/2],lag[n*9/10],lag[n*99/100],lag[n-1],over20,over50,rt[n/2],rt[n*99/100],rt[n-1]);
  return 0;
}

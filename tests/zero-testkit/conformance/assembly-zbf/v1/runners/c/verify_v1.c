#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define MAX_FILE (16u * 1024u * 1024u)
#define HEADER 192u

typedef struct { uint32_t h[8]; uint64_t bits; uint8_t block[64]; size_t used; } sha256_ctx;
static const uint32_t K[64] = {
  0x428a2f98u,0x71374491u,0xb5c0fbcfu,0xe9b5dba5u,0x3956c25bu,0x59f111f1u,0x923f82a4u,0xab1c5ed5u,
  0xd807aa98u,0x12835b01u,0x243185beu,0x550c7dc3u,0x72be5d74u,0x80deb1feu,0x9bdc06a7u,0xc19bf174u,
  0xe49b69c1u,0xefbe4786u,0x0fc19dc6u,0x240ca1ccu,0x2de92c6fu,0x4a7484aau,0x5cb0a9dcu,0x76f988dau,
  0x983e5152u,0xa831c66du,0xb00327c8u,0xbf597fc7u,0xc6e00bf3u,0xd5a79147u,0x06ca6351u,0x14292967u,
  0x27b70a85u,0x2e1b2138u,0x4d2c6dfcu,0x53380d13u,0x650a7354u,0x766a0abbu,0x81c2c92eu,0x92722c85u,
  0xa2bfe8a1u,0xa81a664bu,0xc24b8b70u,0xc76c51a3u,0xd192e819u,0xd6990624u,0xf40e3585u,0x106aa070u,
  0x19a4c116u,0x1e376c08u,0x2748774cu,0x34b0bcb5u,0x391c0cb3u,0x4ed8aa4au,0x5b9cca4fu,0x682e6ff3u,
  0x748f82eeu,0x78a5636fu,0x84c87814u,0x8cc70208u,0x90befffau,0xa4506cebu,0xbef9a3f7u,0xc67178f2u
};
static uint32_t rotr(uint32_t x, unsigned n) { return (x >> n) | (x << (32u - n)); }
static uint32_t be32(const uint8_t *p) { return ((uint32_t)p[0]<<24)|((uint32_t)p[1]<<16)|((uint32_t)p[2]<<8)|p[3]; }
static uint64_t be64(const uint8_t *p) { uint64_t v=0; for (int i=0;i<8;i++) v=(v<<8)|p[i]; return v; }
static void transform(sha256_ctx *c, const uint8_t *b) {
  uint32_t w[64],a,bv,d,e,f,g,h,t1,t2,cc;
  for(int i=0;i<16;i++) w[i]=be32(b+4*i);
  for(int i=16;i<64;i++){uint32_t s0=rotr(w[i-15],7)^rotr(w[i-15],18)^(w[i-15]>>3);uint32_t s1=rotr(w[i-2],17)^rotr(w[i-2],19)^(w[i-2]>>10);w[i]=w[i-16]+s0+w[i-7]+s1;}
  a=c->h[0];bv=c->h[1];cc=c->h[2];d=c->h[3];e=c->h[4];f=c->h[5];g=c->h[6];h=c->h[7];
  for(int i=0;i<64;i++){uint32_t s1=rotr(e,6)^rotr(e,11)^rotr(e,25);uint32_t ch=(e&f)^((~e)&g);t1=h+s1+ch+K[i]+w[i];uint32_t s0=rotr(a,2)^rotr(a,13)^rotr(a,22);uint32_t maj=(a&bv)^(a&cc)^(bv&cc);t2=s0+maj;h=g;g=f;f=e;e=d+t1;d=cc;cc=bv;bv=a;a=t1+t2;}
  c->h[0]+=a;c->h[1]+=bv;c->h[2]+=cc;c->h[3]+=d;c->h[4]+=e;c->h[5]+=f;c->h[6]+=g;c->h[7]+=h;
}
static void sha_init(sha256_ctx *c){uint32_t h[8]={0x6a09e667u,0xbb67ae85u,0x3c6ef372u,0xa54ff53au,0x510e527fu,0x9b05688cu,0x1f83d9abu,0x5be0cd19u};memcpy(c->h,h,sizeof(h));c->bits=0;c->used=0;}
static void sha_update(sha256_ctx *c,const uint8_t *p,size_t n){c->bits+=(uint64_t)n*8u;while(n){size_t take=64-c->used;if(take>n)take=n;memcpy(c->block+c->used,p,take);c->used+=take;p+=take;n-=take;if(c->used==64){transform(c,c->block);c->used=0;}}}
static void sha_final(sha256_ctx *c,uint8_t out[32]){c->block[c->used++]=0x80;if(c->used>56){memset(c->block+c->used,0,64-c->used);transform(c,c->block);c->used=0;}memset(c->block+c->used,0,56-c->used);for(int i=0;i<8;i++)c->block[63-i]=(uint8_t)(c->bits>>(8*i));transform(c,c->block);for(int i=0;i<8;i++){out[4*i]=(uint8_t)(c->h[i]>>24);out[4*i+1]=(uint8_t)(c->h[i]>>16);out[4*i+2]=(uint8_t)(c->h[i]>>8);out[4*i+3]=(uint8_t)c->h[i];}}
static void hash(const uint8_t *p,size_t n,uint8_t out[32]){sha256_ctx c;sha_init(&c);sha_update(&c,p,n);sha_final(&c,out);}
static int hex_eq(const uint8_t d[32],const char *s){static const char x[]="0123456789abcdef";if(strlen(s)!=64)return 0;for(int i=0;i<32;i++)if(s[2*i]!=x[d[i]>>4]||s[2*i+1]!=x[d[i]&15])return 0;return 1;}
static uint8_t *read_file(const char *path,size_t *len){FILE *f=fopen(path,"rb");if(!f)return NULL;if(fseek(f,0,SEEK_END)!=0){fclose(f);return NULL;}long n=ftell(f);if(n<0||(unsigned long)n>MAX_FILE){fclose(f);return NULL;}rewind(f);uint8_t *p=malloc(n?(size_t)n:1);if(!p){fclose(f);return NULL;}if(fread(p,1,(size_t)n,f)!=(size_t)n){free(p);fclose(f);return NULL;}fclose(f);*len=(size_t)n;return p;}
static int verify_hash_file(const char *path,const char *expected,uint8_t **bytes,size_t *len){uint8_t d[32];*bytes=read_file(path,len);if(!*bytes)return 0;hash(*bytes,*len,d);return hex_eq(d,expected);}
static int verify_zbf(const uint8_t *p,size_t n){if(n<HEADER||memcmp(p,"ZEROZBF1",8)!=0)return 0;if(p[8]!=0||p[9]!=1||p[10]!=0||p[11]!=0)return 0;if((p[15]&0xfeu)!=0)return 0;uint64_t payload=be64(p+16);if(payload>(uint64_t)(MAX_FILE-HEADER)||payload+HEADER!=n)return 0;for(size_t i=184;i<192;i++)if(p[i]!=0)return 0;uint8_t d[32];hash(p+HEADER,(size_t)payload,d);return memcmp(d,p+152,32)==0;}
int main(int argc,char **argv){
  if(argc!=8){fprintf(stderr,"usage: verify manifest manifest_sha manifest_digest leaf leaf_sha container container_sha\n");return 64;}
  uint8_t *manifest=NULL,*leaf=NULL,*container=NULL;size_t mn=0,ln=0,cn=0;uint8_t d[32];
  if(!verify_hash_file(argv[1],argv[2],&manifest,&mn))goto fail;
  const uint8_t domain[]="zerostack.assembly_manifest.v1";sha256_ctx c;sha_init(&c);sha_update(&c,domain,sizeof(domain));sha_update(&c,manifest,mn);sha_final(&c,d);if(!hex_eq(d,argv[3]))goto fail;
  if(!verify_hash_file(argv[4],argv[5],&leaf,&ln)||!verify_zbf(leaf,ln))goto fail;
  if(!verify_hash_file(argv[6],argv[7],&container,&cn)||!verify_zbf(container,cn))goto fail;
  free(manifest);free(leaf);free(container);puts("assembly_zbf_kat:c:v1:passed");return 0;
fail: free(manifest);free(leaf);free(container);fputs("fixture_digest_mismatch\n",stderr);return 2;
}

#pragma once

extern const char *g_opcode_string[];

// OP, FUNC, STR, DST, CLASSIFICATION
enum opcode_t {
  ABS_OP = 1,     // abs_impl,"abs",1,1)
  ADD_OP,         // add_impl,"add",1,1)
  ADDP_OP,        // addp_impl,"addp",1,1)
  ADDC_OP,        // addc_impl,"addc",1,1)
  AND_OP,         // and_impl,"and",1,1)
  ANDN_OP,        // andn_impl,"andn",1,1)
  ATOM_OP,        // atom_impl,"atom",1,3)
  BAR_OP,         // bar_impl,"bar",1,3)
  BFE_OP,         // bfe_impl,"bfe",1,1)
  BFI_OP,         // bfi_impl,"bfi",1,1)
  BFIND_OP,       // bfind_impl,"bfind",1,1)
  BRA_OP,         // bra_impl,"bra",0,3)
  BRX_OP,         // brx_impl,"brx",0,3)
  BREV_OP,        // brev_impl,"brev",1,1)
  BRKPT_OP,       // brkpt_impl,"brkpt",1,9)
  MMA_OP,         // mma_impl,"mma",1,1)
  MMA_LD_OP,      // mma_ld_impl,"mma_load",1,5)
  MMA_ST_OP,      // mma_st_impl,"mma_store",0,5)
  CALL_OP,        // call_impl,"call",1,3)
  CALLP_OP,       // callp_impl,"callp",1,3)
  CLZ_OP,         // clz_impl,"clz",1,1)
  CNOT_OP,        // cnot_impl,"cnot",1,1)
  COS_OP,         // cos_impl,"cos",1,4)
  CVT_OP,         // cvt_impl,"cvt",1,1)
  CVTA_OP,        // cvta_impl,"cvta",1,1)
  DIV_OP,         // div_impl,"div",1,1)
  DP4A_OP,        // dp4a_impl,"dp4a",1,1)
  EX2_OP,         // ex2_impl,"ex2",1,4)
  EXIT_OP,        // exit_impl,"exit",1,3)
  FMA_OP,         // fma_impl,"fma",1,2)
  ISSPACEP_OP,    // isspacep_impl,"isspacep",1,1)
  LD_OP,          // ld_impl,"ld",1,5)
  LDU_OP,         // ldu_impl,"ldu",1,5)
  LG2_OP,         // lg2_impl,"lg2",1,4)
  MAD24_OP,       // mad24_impl,"mad24",1,2)
  MAD_OP,         // mad_impl,"mad",1,2)
  MADC_OP,        // madc_impl,"madc",1,2)
  MADP_OP,        // madp_impl,"madp",1,2)
  MAX_OP,         // max_impl,"max",1,1)
  MEMBAR_OP,      // membar_impl,"membar",1,3)
  MIN_OP,         // min_impl,"min",1,1)
  MOV_OP,         // mov_impl,"mov",1,1)
  MUL24_OP,       // mul24_impl,"mul24",1,1)
  MUL_OP,         // mul_impl,"mul",1,1)
  NEG_OP,         // neg_impl,"neg",1,1)
  NANDN_OP,       // nandn_impl,"nandn",1,1)
  NORN_OP,        // norn_impl,"norn",1,1)
  NOT_OP,         // not_impl,"not",1,1)
  OR_OP,          // or_impl,"or",1,1)
  ORN_OP,         // orn_impl,"orn",1,1)
  PMEVENT_OP,     // pmevent_impl,"pmevent",1,10)
  POPC_OP,        // popc_impl,"popc",1,1)
  PREFETCH_OP,    // prefetch_impl,"prefetch",1,5)
  PREFETCHU_OP,   // prefetchu_impl,"prefetchu",1,5)
  PRMT_OP,        // prmt_impl,"prmt",1,1)
  RCP_OP,         // rcp_impl,"rcp",1,4)
  RED_OP,         // red_impl,"red",1,7)
  REM_OP,         // rem_impl,"rem",1,1)
  RET_OP,         // ret_impl,"ret",0,3)
  RETP_OP,        // retp_impl,"retp",0,3)
  RSQRT_OP,       // rsqrt_impl,"rsqrt",1,4)
  SAD_OP,         // sad_impl,"sad",1,1)
  SELP_OP,        // selp_impl,"selp",1,1)
  SETP_OP,        // setp_impl,"setp",1,1)
  SET_OP,         // set_impl,"set",1,1)
  SHFL_OP,        // shfl_impl,"shfl",1,10)
  SHL_OP,         // shl_impl,"shl",1,1)
  SHR_OP,         // shr_impl,"shr",1,1)
  SIN_OP,         // sin_impl,"sin",1,4)
  SLCT_OP,        // slct_impl,"slct",1,1)
  SQRT_OP,        // sqrt_impl,"sqrt",1,4)
  SST_OP,         // sst_impl,"sst",1,5)
  SSY_OP,         // ssy_impl,"ssy",0,3)
  ST_OP,          // st_impl,"st",0,5)
  SUB_OP,         // sub_impl,"sub",1,1)
  SUBC_OP,        // subc_impl,"subc",1,1)
  SULD_OP,        // suld_impl,"suld",1,6)
  SURED_OP,       // sured_impl,"sured",1,6)
  SUST_OP,        // sust_impl,"sust",1,6)
  SUQ_OP,         // suq_impl,"suq",1,6)
  TEX_OP,         // tex_impl,"tex",1,6)
  TRAP_OP,        // trap_impl,"trap",1,3)
  VABSDIFF_OP,    // vabsdiff_impl,"vabsdiff",0,11)
  VADD_OP,        // vadd_impl,"vadd",0,11)
  VMAD_OP,        // vmad_impl,"vmad",0,11)
  VMAX_OP,        // vmax_impl,"vmax",0,11)
  VMIN_OP,        // vmin_impl,"vmin",0,11)
  VSET_OP,        // vset_impl,"vset",0,11)
  VSHL_OP,        // vshl_impl,"vshl",0,11)
  VSHR_OP,        // vshr_impl,"vshr",0,11)
  VSUB_OP,        // vsub_impl,"vsub",0,11)
  VOTE_OP,        // vote_impl,"vote",0,3)
  ACTIVEMASK_OP,  // activemask_impl,"activemask",1,3)
  XOR_OP,         // xor_impl,"xor",1,1)
  NOP_OP,         // nop_impl,"nop",0,7)
  BREAK_OP,       // break_impl,"break",0,3)
  BREAKADDR_OP,   // breakaddr_impl,"breakaddr",0,3)
  NUM_OPCODES
};

use super::Handle;
use crate::runtime::scheduler::telekio::host_schedule;

host_schedule!(CurrentThread, MultiThread);
